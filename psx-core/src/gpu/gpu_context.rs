use crossbeam::channel::Sender;
use vulkano::{
    buffer::{Buffer, BufferContents, BufferCreateInfo, BufferUsage},
    command_buffer::{
        allocator::StandardCommandBufferAllocator, AutoCommandBufferBuilder, BufferImageCopy,
        ClearAttachment, ClearColorImageInfo, ClearRect, CommandBufferInheritanceInfo,
        CommandBufferUsage, CopyBufferToImageInfo, CopyImageInfo, CopyImageToBufferInfo, ImageCopy,
        PrimaryAutoCommandBuffer, PrimaryCommandBufferAbstract, RenderPassBeginInfo,
        SubpassContents,
    },
    descriptor_set::{
        allocator::StandardDescriptorSetAllocator, PersistentDescriptorSet, WriteDescriptorSet,
    },
    device::{Device, Queue},
    format::{ClearColorValue, Format},
    image::{
        view::{ImageView, ImageViewCreateInfo},
        ImageAccess, ImageCreateFlags, ImageDimensions, ImageUsage, StorageImage,
    },
    memory::allocator::{AllocationCreateInfo, MemoryUsage, StandardMemoryAllocator},
    pipeline::{
        graphics::{
            color_blend::{
                AttachmentBlend, BlendFactor, BlendOp, ColorBlendAttachmentState, ColorBlendState,
                ColorComponents,
            },
            input_assembly::{InputAssemblyState, PrimitiveTopology},
            render_pass::PipelineRenderPassType::BeginRenderPass,
            vertex_input::Vertex,
            viewport::{Viewport, ViewportState},
        },
        GraphicsPipeline, Pipeline, PipelineBindPoint, StateMode,
    },
    render_pass::{Framebuffer, FramebufferCreateInfo, Subpass},
    sampler::{
        ComponentMapping, ComponentSwizzle, Filter, Sampler, SamplerAddressMode, SamplerCreateInfo,
        SamplerMipmapMode,
    },
    sync::{self, GpuFuture},
};

use super::front_blit::FrontBlit;
use super::GpuStateSnapshot;

use std::ops::Range;
use std::sync::Arc;

mod vs {
    vulkano_shaders::shader! {
        ty: "vertex",
        path: "src/gpu/shaders/vertex.glsl",
    }
}

mod fs {
    vulkano_shaders::shader! {
        ty: "fragment",
        path: "src/gpu/shaders/fragment.glsl"
    }
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
#[repr(C)]
pub struct DrawingVertex {
    position: [f32; 2],
    color: [f32; 3],
    tex_coord: [i32; 2],
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
    pub fn tex_coord(&mut self) -> [i32; 2] {
        self.tex_coord
    }

    #[inline]
    pub fn set_tex_coord(&mut self, tex_coord: [i32; 2]) {
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
        self.tex_coord = [(tex_coord & 0xFF) as i32, ((tex_coord >> 8) & 0xFF) as i32];
    }
}

/// Contains the vertex data `position, color, tex_coord`, as well as
/// data that is global to the whole polygon/polyline, and were normally sent through
/// `push_constants`, but after using polygon/polyline draw buffering, it would be better
/// to group them into the vertex data.
///
/// TODO: some old GPUs might not support more than 8 vertex data, check on that.
///       We can group multiple data together into a single u32.
#[derive(Copy, Clone, Debug, Default, Vertex, BufferContents)]
#[repr(C)]
struct DrawingVertexFull {
    #[format(R32G32_SFLOAT)]
    position: [f32; 2],
    #[format(R32G32B32_SFLOAT)]
    color: [f32; 3],
    #[format(R32G32_SINT)]
    tex_coord: [i32; 2],

    #[format(R32G32_UINT)]
    clut_base: [u32; 2],
    #[format(R32G32_UINT)]
    tex_page_base: [u32; 2],
    #[format(R32_UINT)]
    semi_transparency_mode: u32, // u8
    #[format(R32_UINT)]
    tex_page_color_mode: u32, // u8

    #[format(R32_UINT)]
    semi_transparent: u32, // bool
    #[format(R32_UINT)]
    dither_enabled: u32, // bool
    #[format(R32_UINT)]
    is_textured: u32, // bool
    #[format(R32_UINT)]
    is_texture_blended: u32, // bool
}

#[derive(Copy, Clone, Debug, Default)]
pub struct DrawingTextureParams {
    pub clut_base: [u32; 2],
    pub tex_page_base: [u32; 2],
    pub semi_transparency_mode: u8,
    pub tex_page_color_mode: u8,
    pub texture_disable: bool,
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
    pub(super) gpu_front_image_sender: Sender<Arc<StorageImage>>,

    pub(super) device: Arc<Device>,
    queue: Arc<Queue>,

    memory_allocator: Arc<StandardMemoryAllocator>,
    command_buffer_allocator: StandardCommandBufferAllocator,

    render_image: Arc<StorageImage>,
    render_image_back_image: Arc<StorageImage>,
    should_update_back_image: bool,

    render_image_framebuffer: Arc<Framebuffer>,
    polygon_pipelines: Vec<Arc<GraphicsPipeline>>,
    polyline_pipelines: Vec<Arc<GraphicsPipeline>>,
    descriptor_set: Arc<PersistentDescriptorSet>,

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
        gpu_front_image_sender: Sender<Arc<StorageImage>>,
    ) -> Self {
        let memory_allocator = Arc::new(StandardMemoryAllocator::new_default(device.clone()));
        let descriptor_set_allocator = StandardDescriptorSetAllocator::new(device.clone());
        let command_buffer_allocator =
            StandardCommandBufferAllocator::new(device.clone(), Default::default());

        let render_image = StorageImage::with_usage(
            &memory_allocator,
            ImageDimensions::Dim2d {
                width: 1024,
                height: 512,
                array_layers: 1,
            },
            Format::A1R5G5B5_UNORM_PACK16,
            ImageUsage::TRANSFER_SRC
                | ImageUsage::TRANSFER_DST
                | ImageUsage::SAMPLED
                | ImageUsage::COLOR_ATTACHMENT,
            ImageCreateFlags::empty(),
            [queue.queue_family_index()],
        )
        .unwrap();

        let render_image_back_image = StorageImage::with_usage(
            &memory_allocator,
            ImageDimensions::Dim2d {
                width: 1024,
                height: 512,
                array_layers: 1,
            },
            Format::A1R5G5B5_UNORM_PACK16,
            ImageUsage::TRANSFER_DST | ImageUsage::SAMPLED,
            ImageCreateFlags::empty(),
            [queue.queue_family_index()],
        )
        .unwrap();

        let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
            AutoCommandBufferBuilder::primary(
                &command_buffer_allocator,
                queue.queue_family_index(),
                CommandBufferUsage::OneTimeSubmit,
            )
            .unwrap();

        builder
            .clear_color_image(ClearColorImageInfo::image(render_image.clone()))
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
                    .vertex_input_state(DrawingVertexFull::per_vertex())
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
                    .vertex_input_state(DrawingVertexFull::per_vertex())
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
            &descriptor_set_allocator,
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

        let front_blit = FrontBlit::new(device.clone(), queue.clone(), render_image.clone());

        let gpu_future = Some(image_clear_future.boxed());

        let command_builder = AutoCommandBufferBuilder::primary(
            &command_buffer_allocator,
            queue.queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .unwrap();

        Self {
            gpu_front_image_sender,

            device,
            queue,

            memory_allocator,
            command_buffer_allocator,

            render_image,
            render_image_framebuffer,

            render_image_back_image,
            should_update_back_image: false,

            polygon_pipelines,
            polyline_pipelines,
            descriptor_set,

            buffered_draw_vertices: Vec::new(),
            current_buffered_draws_state: None,

            front_blit,

            gpu_future,

            command_builder,
            buffered_commands: 0,
        }
    }
}

impl GpuContext {
    pub fn write_vram_block(&mut self, block_range: (Range<u32>, Range<u32>), block: &[u16]) {
        self.check_and_flush_buffered_draws(None);

        let left = block_range.0.start;
        let top = block_range.1.start;
        let width = block_range.0.len() as u32;
        let height = block_range.1.len() as u32;

        let buffer = Buffer::from_iter(
            &self.memory_allocator,
            BufferCreateInfo {
                usage: BufferUsage::TRANSFER_SRC,
                ..Default::default()
            },
            AllocationCreateInfo {
                usage: MemoryUsage::Upload,
                ..Default::default()
            },
            block.iter().cloned(),
        )
        .unwrap();

        let overflow_x = left + width > 1024;
        let overflow_y = top + height > 512;
        if overflow_x || overflow_y {
            let stage_image = StorageImage::new(
                &self.memory_allocator,
                ImageDimensions::Dim2d {
                    width,
                    height,
                    array_layers: 1,
                },
                Format::A1R5G5B5_UNORM_PACK16,
                Some(self.queue.queue_family_index()),
            )
            .unwrap();

            self.command_builder
                .copy_buffer_to_image(CopyBufferToImageInfo::buffer_image(
                    buffer,
                    stage_image.clone(),
                ))
                .unwrap();

            // if we are not overflowing in a direction, just keep the old value
            let not_overflowing_width = (1024 - left).min(width);
            let not_overflowing_height = (512 - top).min(height);
            let remaining_width = width - not_overflowing_width;
            let remaining_height = height - not_overflowing_height;

            // copy the not overflowing content
            self.command_builder
                .copy_image(CopyImageInfo {
                    regions: [ImageCopy {
                        src_subresource: stage_image.subresource_layers(),
                        src_offset: [0, 0, 0],
                        dst_subresource: self.render_image.subresource_layers(),
                        dst_offset: [left, top, 0],
                        extent: [not_overflowing_width, not_overflowing_height, 1],
                        ..Default::default()
                    }]
                    .into(),
                    ..CopyImageInfo::images(stage_image.clone(), self.render_image.clone())
                })
                .unwrap();

            if overflow_x {
                self.command_builder
                    .copy_image(CopyImageInfo {
                        regions: [ImageCopy {
                            src_subresource: stage_image.subresource_layers(),
                            src_offset: [not_overflowing_width, 0, 0],
                            dst_subresource: self.render_image.subresource_layers(),
                            dst_offset: [0, top, 0],
                            extent: [remaining_width, not_overflowing_height, 1],
                            ..Default::default()
                        }]
                        .into(),
                        ..CopyImageInfo::images(stage_image.clone(), self.render_image.clone())
                    })
                    .unwrap();
            }
            if overflow_y {
                self.command_builder
                    .copy_image(CopyImageInfo {
                        regions: [ImageCopy {
                            src_subresource: stage_image.subresource_layers(),
                            src_offset: [0, not_overflowing_height, 0],
                            dst_subresource: self.render_image.subresource_layers(),
                            dst_offset: [left, 0, 0],
                            extent: [not_overflowing_width, remaining_height, 1],
                            ..Default::default()
                        }]
                        .into(),
                        ..CopyImageInfo::images(stage_image.clone(), self.render_image.clone())
                    })
                    .unwrap();
            }
            if overflow_x && overflow_y {
                self.command_builder
                    .copy_image(CopyImageInfo {
                        regions: [ImageCopy {
                            src_subresource: stage_image.subresource_layers(),
                            src_offset: [not_overflowing_width, not_overflowing_height, 0],
                            dst_subresource: self.render_image.subresource_layers(),
                            dst_offset: [0, 0, 0],
                            extent: [remaining_width, remaining_height, 1],
                            ..Default::default()
                        }]
                        .into(),
                        ..CopyImageInfo::images(stage_image, self.render_image.clone())
                    })
                    .unwrap();
            }
        } else {
            self.command_builder
                .copy_buffer_to_image(CopyBufferToImageInfo {
                    regions: [BufferImageCopy {
                        image_subresource: self.render_image.subresource_layers(),
                        image_offset: [left, top, 0],
                        image_extent: [width, height, 1],
                        ..Default::default()
                    }]
                    .into(),
                    ..CopyBufferToImageInfo::buffer_image(buffer, self.render_image.clone())
                })
                .unwrap();
        }

        self.increment_command_builder_commands_and_flush();

        // update back image when loading textures
        self.schedule_back_image_update();
    }

    pub fn read_vram_block(&mut self, block_range: (Range<u32>, Range<u32>)) -> Vec<u16> {
        self.check_and_flush_buffered_draws(None);
        self.flush_command_builder();

        let left = block_range.0.start;
        let top = block_range.1.start;
        let width = block_range.0.len() as u32;
        let height = block_range.1.len() as u32;

        let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
            AutoCommandBufferBuilder::primary(
                &self.command_buffer_allocator,
                self.queue.queue_family_index(),
                CommandBufferUsage::OneTimeSubmit,
            )
            .unwrap();

        let buffer = Buffer::new_slice::<u16>(
            &self.memory_allocator,
            BufferCreateInfo {
                usage: BufferUsage::TRANSFER_DST,
                ..Default::default()
            },
            AllocationCreateInfo {
                usage: MemoryUsage::Download,
                ..Default::default()
            },
            (width * height) as u64,
        )
        .unwrap();

        let overflow_x = left + width > 1024;
        let overflow_y = top + height > 512;
        if overflow_x || overflow_y {
            let stage_image = StorageImage::new(
                &self.memory_allocator,
                ImageDimensions::Dim2d {
                    width,
                    height,
                    array_layers: 1,
                },
                Format::A1R5G5B5_UNORM_PACK16,
                Some(self.queue.queue_family_index()),
            )
            .unwrap();

            // if we are not overflowing in a direction, just keep the old value
            let not_overflowing_width = (1024 - left).min(width);
            let not_overflowing_height = (512 - top).min(height);
            let remaining_width = width - not_overflowing_width;
            let remaining_height = height - not_overflowing_height;

            // copy the not overflowing content
            builder
                .copy_image(CopyImageInfo {
                    regions: [ImageCopy {
                        src_subresource: self.render_image.subresource_layers(),
                        src_offset: [left, top, 0],
                        dst_subresource: stage_image.subresource_layers(),
                        dst_offset: [0, 0, 0],
                        extent: [not_overflowing_width, not_overflowing_height, 1],
                        ..Default::default()
                    }]
                    .into(),
                    ..CopyImageInfo::images(self.render_image.clone(), stage_image.clone())
                })
                .unwrap();

            if overflow_x {
                builder
                    .copy_image(CopyImageInfo {
                        regions: [ImageCopy {
                            src_subresource: self.render_image.subresource_layers(),
                            src_offset: [0, top, 0],
                            dst_subresource: stage_image.subresource_layers(),
                            dst_offset: [not_overflowing_width, 0, 0],
                            extent: [remaining_width, not_overflowing_height, 1],
                            ..Default::default()
                        }]
                        .into(),
                        ..CopyImageInfo::images(self.render_image.clone(), stage_image.clone())
                    })
                    .unwrap();
            }
            if overflow_y {
                builder
                    .copy_image(CopyImageInfo {
                        regions: [ImageCopy {
                            src_subresource: self.render_image.subresource_layers(),
                            src_offset: [left, 0, 0],
                            dst_subresource: stage_image.subresource_layers(),
                            dst_offset: [0, not_overflowing_height, 0],
                            extent: [not_overflowing_width, remaining_height, 1],
                            ..Default::default()
                        }]
                        .into(),
                        ..CopyImageInfo::images(self.render_image.clone(), stage_image.clone())
                    })
                    .unwrap();
            }
            if overflow_x && overflow_y {
                builder
                    .copy_image(CopyImageInfo {
                        regions: [ImageCopy {
                            src_subresource: self.render_image.subresource_layers(),
                            src_offset: [0, 0, 0],
                            dst_subresource: stage_image.subresource_layers(),
                            dst_offset: [not_overflowing_width, not_overflowing_height, 0],
                            extent: [remaining_width, remaining_height, 1],
                            ..Default::default()
                        }]
                        .into(),
                        ..CopyImageInfo::images(self.render_image.clone(), stage_image.clone())
                    })
                    .unwrap();
            }

            builder
                .copy_image_to_buffer(CopyImageToBufferInfo::image_buffer(
                    stage_image,
                    buffer.clone(),
                ))
                .unwrap();
        } else {
            builder
                .copy_image_to_buffer(CopyImageToBufferInfo {
                    regions: [BufferImageCopy {
                        image_subresource: self.render_image.subresource_layers(),
                        image_offset: [left, top, 0],
                        image_extent: [width, height, 1],
                        ..Default::default()
                    }]
                    .into(),
                    ..CopyImageToBufferInfo::image_buffer(self.render_image.clone(), buffer.clone())
                })
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

        self.command_builder
            .begin_render_pass(
                RenderPassBeginInfo {
                    clear_values: vec![None],
                    ..RenderPassBeginInfo::framebuffer(self.render_image_framebuffer.clone())
                },
                SubpassContents::Inline,
            )
            .unwrap()
            .clear_attachments(
                [ClearAttachment::Color {
                    color_attachment: 0,
                    clear_value: ClearColorValue::Float([
                        color.0 as f32 / 255.0,
                        color.1 as f32 / 255.0,
                        color.2 as f32 / 255.0,
                        0.0,
                    ]),
                }],
                [ClearRect {
                    offset: [top_left.0, top_left.1],
                    extent: [width, height],
                    array_layers: 0..1,
                }],
            )
            .unwrap()
            .end_render_pass()
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
                color_write_mask: ColorComponents::R | ColorComponents::G | ColorComponents::B,
                color_write_enable: StateMode::Fixed(true),
            }],
            blend_constants: match semi_transparency_mode {
                4 => StateMode::Fixed([0.0, 0.0, 0.0, 0.25]),
                _ => StateMode::Fixed([0.0, 0.0, 0.0, 0.0]),
            },
        }
    }

    fn new_command_buffer_builder(&mut self) -> AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> {
        let builder = AutoCommandBufferBuilder::primary(
            &self.command_buffer_allocator,
            self.queue.queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .unwrap();

        builder
    }

    fn schedule_back_image_update(&mut self) {
        self.should_update_back_image = true;
    }

    fn update_back_image_if_needed(&mut self) {
        // copy to the back buffer
        if self.should_update_back_image {
            self.should_update_back_image = false;
            self.command_builder
                .copy_image(CopyImageInfo::images(
                    self.render_image.clone(),
                    self.render_image_back_image.clone(),
                ))
                .unwrap();
        }
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
        let vertex_buffer = Buffer::from_iter(
            &self.memory_allocator,
            BufferCreateInfo {
                usage: BufferUsage::VERTEX_BUFFER,
                ..Default::default()
            },
            AllocationCreateInfo {
                usage: MemoryUsage::Upload,
                ..Default::default()
            },
            self.buffered_draw_vertices.iter().cloned(),
        )
        .unwrap();

        let pipelines_set = match current_state.draw_type {
            DrawType::Polygon => &self.polygon_pipelines,
            DrawType::Polyline => &self.polyline_pipelines,
        };
        let pipeline = &pipelines_set[current_state.semi_transparency_mode as usize];

        let push_constants = vs::PushConstantData {
            offset: [
                current_state.drawing_offset.0,
                current_state.drawing_offset.1,
            ],
            drawing_top_left: [current_state.left, current_state.top],
            drawing_size: [current_state.width, current_state.height],
        };

        let subpass = match pipeline.render_pass() {
            BeginRenderPass(subpass) => subpass.clone(),
            _ => unreachable!(),
        };

        let mut secondary_buffer = AutoCommandBufferBuilder::secondary(
            &self.command_buffer_allocator,
            self.queue.queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
            CommandBufferInheritanceInfo {
                render_pass: Some(subpass.into()),
                ..Default::default()
            },
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
                RenderPassBeginInfo {
                    clear_values: vec![None],
                    ..RenderPassBeginInfo::framebuffer(self.render_image_framebuffer.clone())
                },
                SubpassContents::SecondaryCommandBuffers,
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
        texture_params: DrawingTextureParams,
        textured: bool,
        texture_blending: bool,
        semi_transparent: bool,
        state_snapshot: GpuStateSnapshot,
    ) {
        let gpu_stat = state_snapshot.gpu_stat;

        let (drawing_left, drawing_top) = state_snapshot.drawing_area_top_left;
        let (drawing_right, drawing_bottom) = state_snapshot.drawing_area_bottom_right;
        let drawing_offset = state_snapshot.drawing_offset;

        let left = drawing_left;
        let top = drawing_top;
        let height = drawing_bottom + 1 - drawing_top;
        let width = drawing_right + 1 - drawing_left;

        drop(state_snapshot);

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
                self.schedule_back_image_update();
                semi_transparent_mode_3 = true;
            }
        } else {
            // setting semi_transparency_mode to 3 to disable blending since we don't need it
            // mode 3 has no alpha blending, and semi_transparency is handled entirely by
            // the shader.
            semi_transparency_mode = 3;
        }

        // update back image only if we are going to use it
        if textured || semi_transparent_mode_3 {
            self.update_back_image_if_needed();
        }

        // flush previous draws if this is a different state
        self.check_and_flush_buffered_draws(Some(BufferedDrawsState {
            semi_transparency_mode,
            draw_type,
            drawing_offset,
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

    pub(super) fn draw_polygon(
        &mut self,
        vertices: &[DrawingVertex],
        texture_params: DrawingTextureParams,
        textured: bool,
        texture_blending: bool,
        semi_transparent: bool,
        state_snapshot: GpuStateSnapshot,
    ) {
        self.draw(
            vertices,
            DrawType::Polygon,
            texture_params,
            textured,
            texture_blending,
            semi_transparent,
            state_snapshot,
        );
    }

    pub(super) fn draw_polyline(
        &mut self,
        vertices: &[DrawingVertex],
        semi_transparent: bool,
        state_snapshot: GpuStateSnapshot,
    ) {
        // Textures are not supported for polylines
        self.draw(
            vertices,
            DrawType::Polyline,
            DrawingTextureParams::default(),
            false,
            false,
            semi_transparent,
            state_snapshot,
        );
    }

    pub(super) fn blit_to_front(&mut self, full_vram: bool, state_snapshot: GpuStateSnapshot) {
        let gpu_stat = state_snapshot.gpu_stat;
        let vram_display_area_start = state_snapshot.vram_display_area_start;
        self.check_and_flush_buffered_draws(None);
        self.flush_command_builder();

        let (topleft, size) = if full_vram {
            ([0; 2], [1024, 512])
        } else {
            (
                [vram_display_area_start.0, vram_display_area_start.1],
                [
                    gpu_stat.horizontal_resolution(),
                    gpu_stat.vertical_resolution(),
                ],
            )
        };

        let front_image = StorageImage::with_usage(
            &self.memory_allocator,
            ImageDimensions::Dim2d {
                width: size[0],
                height: size[1],
                array_layers: 1,
            },
            Format::B8G8R8A8_UNORM,
            ImageUsage::TRANSFER_SRC | ImageUsage::COLOR_ATTACHMENT,
            ImageCreateFlags::empty(),
            Some(self.queue.queue_family_index()),
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
