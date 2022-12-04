use bytemuck::{Pod, Zeroable};
use crossbeam::atomic::AtomicCell;
use crossbeam::channel::Sender;
use vulkano::buffer::{BufferUsage, CpuAccessibleBuffer, CpuBufferPool};
use vulkano::command_buffer::{
    AutoCommandBufferBuilder, CommandBufferUsage, PrimaryAutoCommandBuffer, PrimaryCommandBuffer,
    SubpassContents,
};
use vulkano::descriptor_set::{PersistentDescriptorSet, WriteDescriptorSet};
use vulkano::device::{Device, Queue};
use vulkano::format::{ClearValue, Format};
use vulkano::image::view::{ImageView, ImageViewCreateInfo};
use vulkano::image::{ImageCreateFlags, ImageDimensions, ImageUsage, StorageImage};
use vulkano::pipeline::graphics::color_blend::{
    AttachmentBlend, BlendFactor, BlendOp, ColorBlendAttachmentState, ColorBlendState,
    ColorComponents,
};
use vulkano::pipeline::graphics::input_assembly::{InputAssemblyState, PrimitiveTopology};
use vulkano::pipeline::graphics::vertex_input::BuffersDefinition;
use vulkano::pipeline::graphics::viewport::{Viewport, ViewportState};
use vulkano::pipeline::{GraphicsPipeline, Pipeline, PipelineBindPoint, StateMode};
use vulkano::render_pass::{Framebuffer, FramebufferCreateInfo, Subpass};
use vulkano::sampler::{
    ComponentMapping, ComponentSwizzle, Filter, Sampler, SamplerAddressMode, SamplerCreateInfo,
    SamplerMipmapMode,
};
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

#[derive(Copy, Clone, Debug, Default, Pod, Zeroable)]
#[repr(C)]
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

/// Contains the vertex data `position, color, tex_coord`, as well as
/// data that is global to the whole polygon/polyline, and were normally sent through
/// `push_constants`, but after using polygon/polyline draw buffering, it would be better
/// to group them into the vertex data.
///
/// TODO: some old GPUs might not support more than 8 vertex data, check on that.
///       We can group multiple data together into a single u32.
#[derive(Copy, Clone, Debug, Default, Pod, Zeroable)]
#[repr(C)]
struct DrawingVertexFull {
    position: [f32; 2],
    color: [f32; 3],
    tex_coord: [u32; 2],

    clut_base: [u32; 2],
    tex_page_base: [u32; 2],
    semi_transparency_mode: u32, // u8
    tex_page_color_mode: u32,    // u8
    texture_flip: [u32; 2],      // (bool, bool)

    semi_transparent: u32,   // bool
    dither_enabled: u32,     // bool
    is_textured: u32,        // bool
    is_texture_blended: u32, // bool
}

vulkano::impl_vertex!(
    DrawingVertexFull,
    position,
    color,
    tex_coord,
    clut_base,
    tex_page_base,
    semi_transparency_mode,
    tex_page_color_mode,
    texture_flip,
    semi_transparent,
    dither_enabled,
    is_textured,
    is_texture_blended
);

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

/// The type of the draw command, informs how the drawing vertices should be handled
#[derive(PartialEq, Debug)]
pub enum DrawType {
    Polygon,
    Polyline,
}

/// A structure to hold the similar state of consecutive draws.
/// If any of these states got changed, the buffered draws should be flushed
/// and a new state is established with the new values.
#[derive(PartialEq, Debug)]
struct BufferedDrawsState {
    /// Same semi_transparency_mode can use the same pipeline
    semi_transparency_mode: u8,
    /// The type of the vertices buffered
    draw_type: DrawType,
    /// It is used for push constants, and will rarely change
    left: u32,
    /// It is used for push constants, and will rarely change
    top: u32,
    /// It is used for push constants, and will rarely change
    width: u32,
    /// It is used for push constants, and will rarely change
    height: u32,
    /// It is used for push constants, and will rarely change
    drawing_offset: (i32, i32),
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
    polygon_pipelines: Vec<Arc<GraphicsPipeline>>,
    polyline_pipelines: Vec<Arc<GraphicsPipeline>>,
    descriptor_set: Arc<PersistentDescriptorSet>,

    vertex_buffer_pool: CpuBufferPool<DrawingVertexFull>,
    buffered_draw_vertices: Vec<DrawingVertexFull>,
    current_buffered_draws_state: Option<BufferedDrawsState>,

    front_blit: FrontBlit,

    gpu_future: Option<Box<dyn GpuFuture>>,

    command_builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer>,
    buffered_commands: u32,
}

impl GpuContext {
    pub(super) fn new(
        device: Arc<Device>,
        queue: Arc<Queue>,
        gpu_stat: Arc<AtomicCell<GpuStat>>,
        gpu_read_sender: Sender<u32>,
        gpu_front_image_sender: Sender<Arc<StorageImage>>,
    ) -> Self {
        let render_image = StorageImage::with_usage(
            device.clone(),
            ImageDimensions::Dim2d {
                width: 1024,
                height: 512,
                array_layers: 1,
            },
            Format::A1R5G5B5_UNORM_PACK16,
            ImageUsage {
                transfer_source: true,
                transfer_destination: true,
                color_attachment: true,
                sampled: true,
                ..ImageUsage::none()
            },
            ImageCreateFlags::none(),
            [queue.family()],
        )
        .unwrap();

        let render_image_back_image = StorageImage::with_usage(
            device.clone(),
            ImageDimensions::Dim2d {
                width: 1024,
                height: 512,
                array_layers: 1,
            },
            Format::A1R5G5B5_UNORM_PACK16,
            ImageUsage {
                transfer_destination: true,
                sampled: true,
                ..ImageUsage::none()
            },
            ImageCreateFlags::none(),
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

        // create multiple pipelines, one for each semi_transparency_mode
        // TODO: is there a better way to do this?
        let polygon_pipelines = (0..5)
            .map(|transparency_mode| {
                GraphicsPipeline::start()
                    .vertex_input_state(BuffersDefinition::new().vertex::<DrawingVertexFull>())
                    .vertex_shader(vs.entry_point("main").unwrap(), ())
                    .input_assembly_state(
                        InputAssemblyState::new().topology(PrimitiveTopology::TriangleList),
                    )
                    .color_blend_state(Self::create_color_blend_state(transparency_mode))
                    .viewport_state(ViewportState::viewport_dynamic_scissor_irrelevant())
                    .fragment_shader(fs.entry_point("main").unwrap(), ())
                    .render_pass(Subpass::from(render_pass.clone(), 0).unwrap())
                    .build(device.clone())
                    .unwrap()
            })
            .collect::<Vec<_>>();

        // multiple pipelines
        let polyline_pipelines = (0..5)
            .map(|transparency_mode| {
                GraphicsPipeline::start()
                    .vertex_input_state(BuffersDefinition::new().vertex::<DrawingVertexFull>())
                    .vertex_shader(vs.entry_point("main").unwrap(), ())
                    .input_assembly_state(
                        InputAssemblyState::new().topology(PrimitiveTopology::LineList),
                    )
                    .color_blend_state(Self::create_color_blend_state(transparency_mode))
                    .viewport_state(ViewportState::viewport_dynamic_scissor_irrelevant())
                    .fragment_shader(fs.entry_point("main").unwrap(), ())
                    .render_pass(Subpass::from(render_pass.clone(), 0).unwrap())
                    .build(device.clone())
                    .unwrap()
            })
            .collect::<Vec<_>>();

        let sampler = Sampler::new(
            device.clone(),
            SamplerCreateInfo {
                mag_filter: Filter::Nearest,
                min_filter: Filter::Nearest,
                mipmap_mode: SamplerMipmapMode::Nearest,
                address_mode: [SamplerAddressMode::Repeat; 3],
                ..Default::default()
            },
        )
        .unwrap();

        // even though, we are using the layout from the `polygon_pipeline`
        // it still works without issues with `line_pipeline` since its the
        // same layout taken from the same shader.
        let layout = polygon_pipelines[0].layout().set_layouts().get(0).unwrap();

        let render_image_back_image_view = ImageView::new(
            render_image_back_image.clone(),
            ImageViewCreateInfo {
                component_mapping: ComponentMapping {
                    r: ComponentSwizzle::Blue,
                    b: ComponentSwizzle::Red,
                    ..Default::default()
                },
                ..ImageViewCreateInfo::from_image(&render_image_back_image)
            },
        )
        .unwrap();

        let descriptor_set = PersistentDescriptorSet::new(
            layout.clone(),
            [WriteDescriptorSet::image_view_sampler(
                0,
                render_image_back_image_view,
                sampler,
            )],
        )
        .unwrap();
        let render_image_framebuffer = Framebuffer::new(
            render_pass,
            FramebufferCreateInfo {
                attachments: vec![ImageView::new_default(render_image.clone()).unwrap()],
                ..Default::default()
            },
        )
        .unwrap();

        let vertex_buffer_pool = CpuBufferPool::vertex_buffer(device.clone());

        let (front_blit, front_blit_init_future) =
            FrontBlit::new(device.clone(), queue.clone(), render_image.clone());

        let gpu_future = Some(image_clear_future.join(front_blit_init_future).boxed());

        let command_builder = AutoCommandBufferBuilder::primary(
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

            polygon_pipelines,
            polyline_pipelines,
            descriptor_set,

            vertex_buffer_pool,
            buffered_draw_vertices: Vec::new(),
            current_buffered_draws_state: None,

            front_blit,

            gpu_future,

            command_builder,
            buffered_commands: 0,
        }
    }

    pub(super) fn read_gpu_stat(&self) -> GpuStat {
        self.gpu_stat.load()
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
        self.check_and_flush_buffered_draws(None);

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

        let overflow_x = left + width > 1024;
        let overflow_y = top + height > 512;
        if overflow_x || overflow_y {
            let stage_image = StorageImage::new(
                self.device.clone(),
                ImageDimensions::Dim2d {
                    width,
                    height,
                    array_layers: 1,
                },
                Format::A1R5G5B5_UNORM_PACK16,
                Some(self.queue.family()),
            )
            .unwrap();

            self.command_builder
                .copy_buffer_to_image(buffer, stage_image.clone())
                .unwrap();

            // if we are not overflowing in a direction, just keep the old value
            let not_overflowing_width = (1024 - left).min(width);
            let not_overflowing_height = (512 - top).min(height);
            let remaining_width = width - not_overflowing_width;
            let remaining_height = height - not_overflowing_height;

            // copy the not overflowing content
            self.command_builder
                .copy_image(
                    stage_image.clone(),
                    [0, 0, 0],
                    0,
                    0,
                    self.render_image.clone(),
                    [left as i32, top as i32, 0],
                    0,
                    0,
                    [not_overflowing_width, not_overflowing_height, 1],
                    1,
                )
                .unwrap();

            if overflow_x {
                self.command_builder
                    .copy_image(
                        stage_image.clone(),
                        [not_overflowing_width as i32, 0, 0],
                        0,
                        0,
                        self.render_image.clone(),
                        [0, top as i32, 0],
                        0,
                        0,
                        [remaining_width, not_overflowing_height, 1],
                        1,
                    )
                    .unwrap();
            }
            if overflow_y {
                self.command_builder
                    .copy_image(
                        stage_image.clone(),
                        [0, not_overflowing_height as i32, 0],
                        0,
                        0,
                        self.render_image.clone(),
                        [left as i32, 0, 0],
                        0,
                        0,
                        [not_overflowing_width, remaining_height, 1],
                        1,
                    )
                    .unwrap();
            }
            if overflow_x && overflow_y {
                self.command_builder
                    .copy_image(
                        stage_image.clone(),
                        [
                            not_overflowing_width as i32,
                            not_overflowing_height as i32,
                            0,
                        ],
                        0,
                        0,
                        self.render_image.clone(),
                        [0, 0, 0],
                        0,
                        0,
                        [remaining_width, remaining_height, 1],
                        1,
                    )
                    .unwrap();
            }
        } else {
            self.command_builder
                .copy_buffer_to_image_dimensions(
                    buffer.clone(),
                    self.render_image.clone(),
                    [left, top, 0],
                    [width, height, 1],
                    0,
                    1,
                    0,
                )
                .unwrap();
        }

        self.increment_command_builder_commands_and_flush();

        // update back image when loading textures
        self.update_back_image();
    }

    pub fn read_vram_block(&mut self, block_range: &(Range<u32>, Range<u32>)) -> Vec<u16> {
        self.check_and_flush_buffered_draws(None);
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

        let overflow_x = left + width > 1024;
        let overflow_y = top + height > 512;
        if overflow_x || overflow_y {
            let stage_image = StorageImage::new(
                self.device.clone(),
                ImageDimensions::Dim2d {
                    width,
                    height,
                    array_layers: 1,
                },
                Format::A1R5G5B5_UNORM_PACK16,
                Some(self.queue.family()),
            )
            .unwrap();

            // if we are not overflowing in a direction, just keep the old value
            let not_overflowing_width = (1024 - left).min(width);
            let not_overflowing_height = (512 - top).min(height);
            let remaining_width = width - not_overflowing_width;
            let remaining_height = height - not_overflowing_height;

            // copy the not overflowing content
            builder
                .copy_image(
                    self.render_image.clone(),
                    [left as i32, top as i32, 0],
                    0,
                    0,
                    stage_image.clone(),
                    [0, 0, 0],
                    0,
                    0,
                    [not_overflowing_width, not_overflowing_height, 1],
                    1,
                )
                .unwrap();

            if overflow_x {
                builder
                    .copy_image(
                        self.render_image.clone(),
                        [0, top as i32, 0],
                        0,
                        0,
                        stage_image.clone(),
                        [not_overflowing_width as i32, 0, 0],
                        0,
                        0,
                        [remaining_width, not_overflowing_height, 1],
                        1,
                    )
                    .unwrap();
            }
            if overflow_y {
                builder
                    .copy_image(
                        self.render_image.clone(),
                        [left as i32, 0, 0],
                        0,
                        0,
                        stage_image.clone(),
                        [0, not_overflowing_height as i32, 0],
                        0,
                        0,
                        [not_overflowing_width, remaining_height, 1],
                        1,
                    )
                    .unwrap();
            }
            if overflow_x && overflow_y {
                builder
                    .copy_image(
                        self.render_image.clone(),
                        [0, 0, 0],
                        0,
                        0,
                        stage_image.clone(),
                        [
                            not_overflowing_width as i32,
                            not_overflowing_height as i32,
                            0,
                        ],
                        0,
                        0,
                        [remaining_width, remaining_height, 1],
                        1,
                    )
                    .unwrap();
            }

            builder
                .copy_image_to_buffer(stage_image, buffer.clone())
                .unwrap();
        } else {
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
        }

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
        let mut width = size.0;
        let mut height = size.1;

        if width * height == 0 {
            return;
        }
        self.check_and_flush_buffered_draws(None);

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
        self.increment_command_builder_commands_and_flush();
    }

    /// Create ColorBlendState for a specific semi_transparency_mode, to be
    /// used to create a specific pipeline for it.
    fn create_color_blend_state(semi_transparency_mode: u8) -> ColorBlendState {
        // Mode 3 has no blend, so it is used for non_transparent draws
        let blend = match semi_transparency_mode {
            0 => Some(AttachmentBlend {
                color_op: BlendOp::Add,
                color_source: BlendFactor::SrcAlpha,
                color_destination: BlendFactor::OneMinusSrcAlpha,
                alpha_op: BlendOp::Add,
                alpha_source: BlendFactor::One,
                alpha_destination: BlendFactor::Zero,
            }),
            1 => Some(AttachmentBlend {
                color_op: BlendOp::Add,
                color_source: BlendFactor::One,
                color_destination: BlendFactor::SrcAlpha,
                alpha_op: BlendOp::Add,
                alpha_source: BlendFactor::One,
                alpha_destination: BlendFactor::Zero,
            }),
            2 => Some(AttachmentBlend {
                color_op: BlendOp::ReverseSubtract,
                color_source: BlendFactor::One,
                color_destination: BlendFactor::SrcAlpha,
                alpha_op: BlendOp::Add,
                alpha_source: BlendFactor::One,
                alpha_destination: BlendFactor::Zero,
            }),
            3 => None,
            // NOTE: this is not a valid semi_transparency_mode, but we
            //       used it to create a faster path for non-textured mode 3
            //
            // faster path for mode 3 non-textured
            4 => Some(AttachmentBlend {
                color_op: BlendOp::Add,
                color_source: BlendFactor::ConstantAlpha,
                color_destination: BlendFactor::One,
                alpha_op: BlendOp::Add,
                alpha_source: BlendFactor::One,
                alpha_destination: BlendFactor::Zero,
            }),
            _ => unreachable!(),
        };
        ColorBlendState {
            logic_op: None,
            attachments: vec![ColorBlendAttachmentState {
                blend,
                color_write_mask: ColorComponents {
                    a: false,
                    ..ColorComponents::all()
                },
                color_write_enable: StateMode::Fixed(true),
            }],
            blend_constants: match semi_transparency_mode {
                4 => StateMode::Fixed([0.0, 0.0, 0.0, 0.25]),
                _ => StateMode::Fixed([0.0, 0.0, 0.0, 0.0]),
            },
        }
    }

    fn new_command_buffer_builder(&mut self) -> AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> {
        let mut builder = AutoCommandBufferBuilder::primary(
            self.device.clone(),
            self.queue.family(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .unwrap();

        // copy to the back buffer
        // NOTE: For some reason, removing this results in conflict error from vulkano
        //       not sure, if there is a bug with the conflict checker or not, but
        //       for now, we add this command in the beginning before binding the image as texture.
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

        builder
    }

    fn update_back_image(&mut self) {
        // copy to the back buffer
        self.command_builder
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
    }

    fn flush_command_builder(&mut self) {
        // No need to flush if there no draw commands
        if self.buffered_commands == 0 {
            return;
        }
        let new_builder = self.new_command_buffer_builder();
        let command_buffer_builder = std::mem::replace(&mut self.command_builder, new_builder);
        self.buffered_commands = 0;

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

    // Checks the `new_state` with the `current_state`, if they are different,
    // it will flush the buffered vertices, and set the `current_state` to `new_state`.
    //
    // Using `None` as `new_state` will always flush the buffered vertices (if any).
    fn check_and_flush_buffered_draws(&mut self, new_state: Option<BufferedDrawsState>) {
        if new_state == self.current_buffered_draws_state {
            return;
        }
        let current_state = std::mem::replace(&mut self.current_buffered_draws_state, new_state);

        let current_state = if let Some(state) = current_state {
            state
        } else {
            return;
        };

        let vertices_len = self.buffered_draw_vertices.len();
        // if we have a valid instance, then there must be some vertices
        assert!(vertices_len > 0);

        // we create a "cloned iter" here so that we don't clone the vector
        let vertex_buffer = self
            .vertex_buffer_pool
            .chunk(self.buffered_draw_vertices.iter().cloned())
            .unwrap();

        let pipelines_set = match current_state.draw_type {
            DrawType::Polygon => &self.polygon_pipelines,
            DrawType::Polyline => &self.polyline_pipelines,
        };
        let pipeline = &pipelines_set[current_state.semi_transparency_mode as usize];

        let push_constants = vs::ty::PushConstantData {
            offset: [
                current_state.drawing_offset.0,
                current_state.drawing_offset.1,
            ],
            drawing_top_left: [current_state.left, current_state.top],
            drawing_size: [current_state.width, current_state.height],
        };

        let mut secondary_buffer = AutoCommandBufferBuilder::secondary_graphics(
            self.device.clone(),
            self.queue.family(),
            CommandBufferUsage::OneTimeSubmit,
            pipeline.subpass().clone(),
        )
        .unwrap();

        secondary_buffer
            .set_viewport(
                0,
                [Viewport {
                    origin: [current_state.left as f32, current_state.top as f32],
                    dimensions: [current_state.width as f32, current_state.height as f32],
                    depth_range: 0.0..1.0,
                }],
            )
            .bind_descriptor_sets(
                PipelineBindPoint::Graphics,
                pipeline.layout().clone(),
                0,
                self.descriptor_set.clone(),
            )
            .bind_pipeline_graphics(pipeline.clone())
            .push_constants(pipeline.layout().clone(), 0, push_constants)
            .bind_vertex_buffers(0, vertex_buffer)
            .draw(vertices_len as u32, 1, 0, 0)
            .unwrap();

        self.command_builder
            .begin_render_pass(
                self.render_image_framebuffer.clone(),
                SubpassContents::SecondaryCommandBuffers,
                [ClearValue::None],
            )
            .unwrap()
            .execute_commands(secondary_buffer.build().unwrap())
            .unwrap()
            .end_render_pass()
            .unwrap();

        self.increment_command_builder_commands_and_flush();

        // prepare for next batch
        self.buffered_draw_vertices.clear();
    }

    /// Adds to the buffered commands counter and flushes the command builder if needed exceeded a
    /// specific threshold.
    fn increment_command_builder_commands_and_flush(&mut self) {
        // NOTE: this number is arbitrary, it should be tested later or maybe
        //       make it dynamic
        const MAX_BUFFERED_COMMANDS: u32 = 20;

        self.buffered_commands += 1;
        if self.buffered_commands > MAX_BUFFERED_COMMANDS {
            self.flush_command_builder();
        }
    }

    /// common function to draw polygons and polylines
    #[inline]
    fn draw(
        &mut self,
        vertices: &[DrawingVertex],
        draw_type: DrawType,
        mut texture_params: DrawingTextureParams,
        textured: bool,
        texture_blending: bool,
        semi_transparent: bool,
    ) {
        let gpu_stat = self.read_gpu_stat();

        let (drawing_left, drawing_top) = self.drawing_area_top_left;
        let (drawing_right, drawing_bottom) = self.drawing_area_bottom_right;

        let left = drawing_left;
        let top = drawing_top;
        let height = drawing_bottom + 1 - drawing_top;
        let width = drawing_right + 1 - drawing_left;

        if textured {
            if !self.allow_texture_disable {
                texture_params.texture_disable = false;
            }
            self.update_gpu_stat_from_texture_params(&texture_params);
        };

        let mut semi_transparency_mode = if textured {
            texture_params.semi_transparency_mode
        } else {
            let s = gpu_stat.semi_transparency_mode();
            if s == 3 {
                4 // special faster path for mode 3 non-textured
            } else {
                s
            }
        };

        let mut semi_transparent_mode_3 = false;
        // we might need to update back image if we are drawing `textured`
        // But, updating textures isn't done a lot, so most of the updates
        // will be not needed. Thus, we don't update if its `textured`
        // TODO: fix texture updates and back image updates
        if semi_transparent {
            if semi_transparency_mode == 3 {
                // flush previous batch because semi_transparent mode 3 cannot be grouped
                // with other draws, since it relies on updated back image
                self.check_and_flush_buffered_draws(None);
                self.update_back_image();
                semi_transparent_mode_3 = true;
            }
        } else {
            // setting semi_transparency_mode to 3 to disable blending since we don't need it
            // mode 3 has no alpha blending, and semi_transparency is handled entirely by
            // the shader.
            semi_transparency_mode = 3;
        }

        // flush previous draws if this is a different state
        self.check_and_flush_buffered_draws(Some(BufferedDrawsState {
            semi_transparency_mode,
            draw_type,
            drawing_offset: self.drawing_offset,
            left,
            top,
            width,
            height,
        }));

        let converted_vertices_iter = vertices.iter().map(|v| DrawingVertexFull {
            position: v.position,
            color: v.color,
            tex_coord: v.tex_coord,
            clut_base: texture_params.clut_base,
            tex_page_base: texture_params.tex_page_base,
            semi_transparency_mode: semi_transparency_mode as u32,
            tex_page_color_mode: texture_params.tex_page_color_mode as u32,
            texture_flip: [
                texture_params.texture_flip.0 as u32,
                texture_params.texture_flip.1 as u32,
            ],
            semi_transparent: semi_transparent as u32,
            dither_enabled: gpu_stat.dither_enabled() as u32,
            is_textured: textured as u32,
            is_texture_blended: texture_blending as u32,
        });
        self.buffered_draw_vertices.extend(converted_vertices_iter);

        if semi_transparent_mode_3 {
            // flush the draw immediately
            self.check_and_flush_buffered_draws(None);
        }
    }

    pub fn draw_polygon(
        &mut self,
        vertices: &[DrawingVertex],
        texture_params: DrawingTextureParams,
        textured: bool,
        texture_blending: bool,
        semi_transparent: bool,
    ) {
        self.draw(
            vertices,
            DrawType::Polygon,
            texture_params,
            textured,
            texture_blending,
            semi_transparent,
        );
    }

    pub fn draw_polyline(&mut self, vertices: &[DrawingVertex], semi_transparent: bool) {
        // Textures are not supported for polylines
        self.draw(
            vertices,
            DrawType::Polyline,
            DrawingTextureParams::default(),
            false,
            false,
            semi_transparent,
        );
    }

    pub fn blit_to_front(&mut self, full_vram: bool) {
        let gpu_stat = self.read_gpu_stat();
        self.check_and_flush_buffered_draws(None);
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

        let front_image = StorageImage::with_usage(
            self.device.clone(),
            ImageDimensions::Dim2d {
                width: size[0],
                height: size[1],
                array_layers: 1,
            },
            Format::B8G8R8A8_UNORM,
            ImageUsage {
                transfer_source: true,
                color_attachment: true,
                ..ImageUsage::none()
            },
            ImageCreateFlags::none(),
            Some(self.queue.family()),
        )
        .unwrap();

        // TODO: try to remove the `wait` from here
        self.front_blit
            .blit(
                front_image.clone(),
                topleft,
                size,
                gpu_stat.is_24bit_color_depth(),
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
