pub use vulkano::{
    buffer::{Buffer, BufferContents, BufferCreateInfo, BufferUsage},
    command_buffer::{
        allocator::StandardCommandBufferAllocator, AutoCommandBufferBuilder, BufferImageCopy,
        ClearAttachment, ClearColorImageInfo, ClearRect, CommandBufferUsage, CopyBufferToImageInfo,
        CopyImageInfo, CopyImageToBufferInfo, ImageCopy, PrimaryAutoCommandBuffer,
        PrimaryCommandBufferAbstract, RenderPassBeginInfo,
    },
    descriptor_set::{
        allocator::StandardDescriptorSetAllocator, DescriptorSet, WriteDescriptorSet,
    },
    device::{Device, Queue},
    format::{ClearColorValue, Format},
    image::{
        sampler::{
            ComponentMapping, ComponentSwizzle, Filter, Sampler, SamplerAddressMode,
            SamplerCreateInfo, SamplerMipmapMode,
        },
        view::{ImageView, ImageViewCreateInfo},
        Image, ImageCreateInfo, ImageType, ImageUsage,
    },
    memory::allocator::{AllocationCreateInfo, MemoryTypeFilter, StandardMemoryAllocator},
    pipeline::{
        graphics::{
            color_blend::{
                AttachmentBlend, BlendFactor, BlendOp, ColorBlendAttachmentState, ColorBlendState,
                ColorComponents,
            },
            input_assembly::{InputAssemblyState, PrimitiveTopology},
            multisample::MultisampleState,
            rasterization::RasterizationState,
            vertex_input::{Vertex, VertexDefinition},
            viewport::{Viewport, ViewportState},
            GraphicsPipelineCreateInfo,
        },
        layout::PipelineDescriptorSetLayoutCreateInfo,
        DynamicState, GraphicsPipeline, Pipeline, PipelineBindPoint, PipelineLayout,
        PipelineShaderStageCreateInfo,
    },
    render_pass::{Framebuffer, FramebufferCreateInfo, Subpass},
    sync::{self, GpuFuture},
};

use super::front_blit::FrontBlit;
use crate::gpu::DrawingTextureParams;
use crate::gpu::DrawingVertex;
use crate::gpu::GpuStateSnapshot;

use std::sync::Arc;
use std::{ops::Range, sync::mpsc};

mod vs {
    vulkano_shaders::shader! {
        ty: "vertex",
        path: "src/gpu/vulkan/gpu/shaders/vertex.glsl",
    }
}

mod fs {
    vulkano_shaders::shader! {
        ty: "fragment",
        path: "src/gpu/vulkan/gpu/shaders/fragment.glsl"
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

    /// group multiple data into one array
    /// clut_base: [u32; 2],
    /// tex_page_base: [u32; 2],
    #[format(R32G32B32A32_UINT)]
    tex_info: [u32; 4],

    /// group multiple data into one array
    /// tex_window_mask: [u32; 2],
    /// tex_window_offset: [u32; 2],
    #[format(R32G32B32A32_UINT)]
    tex_window: [u32; 4],

    /// group multiple data into one array
    ///
    /// semi_transparency_mode: u32,
    /// tex_page_color_mode: u32,
    /// bool_flags: u32,
    ///  bit 0: semi_transparent
    ///  bit 1: dither_enabled
    ///  bit 2: is_textured
    ///  bit 3: is_texture_blended
    #[format(R32G32B32_UINT)]
    extra_draw_state: [u32; 3],
}

impl DrawingVertexFull {
    #[allow(clippy::too_many_arguments)]
    fn new(
        v: &DrawingVertex,
        texture_params: &DrawingTextureParams,
        texture_window_mask: (u32, u32),
        texture_window_offset: (u32, u32),
        semi_transparency_mode: u8,
        semi_transparent: bool,
        dither_enabled: bool,
        textured: bool,
        texture_blending: bool,
    ) -> Self {
        let bool_flags = semi_transparent as u32
            | (dither_enabled as u32) << 1
            | (textured as u32) << 2
            | (texture_blending as u32) << 3;
        Self {
            position: v.position(),
            color: v.color(),
            tex_coord: v.tex_coord(),
            tex_info: [
                texture_params.clut_base[0],
                texture_params.clut_base[1],
                texture_params.tex_page_base[0],
                texture_params.tex_page_base[1],
            ],
            tex_window: [
                texture_window_mask.0,
                texture_window_mask.1,
                texture_window_offset.0,
                texture_window_offset.1,
            ],
            extra_draw_state: [
                semi_transparency_mode as u32,
                texture_params.tex_page_color_mode as u32,
                bool_flags,
            ],
        }
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
    pub(super) gpu_front_image_sender: mpsc::Sender<Arc<Image>>,

    pub(super) device: Arc<Device>,
    queue: Arc<Queue>,

    memory_allocator: Arc<StandardMemoryAllocator>,
    command_buffer_allocator: Arc<StandardCommandBufferAllocator>,

    render_image: Arc<Image>,
    render_image_back_image: Arc<Image>,
    should_update_back_image: bool,

    render_image_framebuffer: Arc<Framebuffer>,
    polygon_pipelines: Vec<Arc<GraphicsPipeline>>,
    polyline_pipelines: Vec<Arc<GraphicsPipeline>>,
    descriptor_set: Arc<DescriptorSet>,

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
        gpu_front_image_sender: mpsc::Sender<Arc<Image>>,
    ) -> Self {
        let memory_allocator = Arc::new(StandardMemoryAllocator::new_default(device.clone()));
        let descriptor_set_allocator = Arc::new(StandardDescriptorSetAllocator::new(
            device.clone(),
            Default::default(),
        ));
        let command_buffer_allocator = Arc::new(StandardCommandBufferAllocator::new(
            device.clone(),
            Default::default(),
        ));

        let render_image = Image::new(
            memory_allocator.clone(),
            ImageCreateInfo {
                image_type: ImageType::Dim2d,
                extent: [1024, 512, 1],
                format: Format::A1R5G5B5_UNORM_PACK16,
                usage: ImageUsage::TRANSFER_SRC
                    | ImageUsage::TRANSFER_DST
                    | ImageUsage::SAMPLED
                    | ImageUsage::COLOR_ATTACHMENT,
                ..Default::default()
            },
            Default::default(),
        )
        .unwrap();

        let render_image_back_image = Image::new(
            memory_allocator.clone(),
            ImageCreateInfo {
                image_type: ImageType::Dim2d,
                extent: [1024, 512, 1],
                format: Format::A1R5G5B5_UNORM_PACK16,
                usage: ImageUsage::TRANSFER_DST | ImageUsage::SAMPLED,
                ..Default::default()
            },
            Default::default(),
        )
        .unwrap();

        let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
            AutoCommandBufferBuilder::primary(
                command_buffer_allocator.clone(),
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

        let vs = vs::load(device.clone())
            .unwrap()
            .entry_point("main")
            .unwrap();
        let fs = fs::load(device.clone())
            .unwrap()
            .entry_point("main")
            .unwrap();

        let render_pass = vulkano::single_pass_renderpass!(
            device.clone(),
            attachments: {
                color: {
                    format: Format::A1R5G5B5_UNORM_PACK16,
                    samples: 1,
                    load_op: Load,
                    store_op: Store,
                }
            },
            pass: {
                color: [color],
                depth_stencil: {},
            }
        )
        .unwrap();

        let vertex_input_state = DrawingVertexFull::per_vertex().definition(&vs).unwrap();
        let stages = [
            PipelineShaderStageCreateInfo::new(vs),
            PipelineShaderStageCreateInfo::new(fs),
        ];

        let layout = PipelineLayout::new(
            device.clone(),
            PipelineDescriptorSetLayoutCreateInfo::from_stages(&stages)
                .into_pipeline_layout_create_info(device.clone())
                .unwrap(),
        )
        .unwrap();
        let subpass = Subpass::from(render_pass.clone(), 0).unwrap();
        // create multiple pipelines, one for each semi_transparency_mode
        // TODO: is there a better way to do this?
        let polygon_pipelines = (0..5)
            .map(|transparency_mode| {
                GraphicsPipeline::new(
                    device.clone(),
                    None,
                    GraphicsPipelineCreateInfo {
                        stages: stages.clone().into_iter().collect(),
                        vertex_input_state: Some(vertex_input_state.clone()),
                        input_assembly_state: Some(InputAssemblyState {
                            topology: PrimitiveTopology::TriangleList,
                            ..Default::default()
                        }),
                        rasterization_state: Some(RasterizationState::default()),
                        multisample_state: Some(MultisampleState::default()),
                        color_blend_state: Some(Self::create_color_blend_state(transparency_mode)),
                        viewport_state: Some(ViewportState::default()),
                        dynamic_state: [DynamicState::Viewport].into_iter().collect(),
                        subpass: Some(subpass.clone().into()),
                        ..GraphicsPipelineCreateInfo::layout(layout.clone())
                    },
                )
                .unwrap()
            })
            .collect::<Vec<_>>();

        // multiple pipelines
        let polyline_pipelines = (0..5)
            .map(|transparency_mode| {
                GraphicsPipeline::new(
                    device.clone(),
                    None,
                    GraphicsPipelineCreateInfo {
                        stages: stages.clone().into_iter().collect(),
                        vertex_input_state: Some(vertex_input_state.clone()),
                        input_assembly_state: Some(InputAssemblyState {
                            topology: PrimitiveTopology::LineList,
                            ..Default::default()
                        }),
                        rasterization_state: Some(RasterizationState::default()),
                        multisample_state: Some(MultisampleState::default()),
                        color_blend_state: Some(Self::create_color_blend_state(transparency_mode)),
                        viewport_state: Some(ViewportState::default()),
                        dynamic_state: [DynamicState::Viewport].into_iter().collect(),
                        subpass: Some(subpass.clone().into()),
                        ..GraphicsPipelineCreateInfo::layout(layout.clone())
                    },
                )
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
        let layout = polygon_pipelines[0].layout().set_layouts().first().unwrap();

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

        let descriptor_set = DescriptorSet::new(
            descriptor_set_allocator.clone(),
            layout.clone(),
            [WriteDescriptorSet::image_view_sampler(
                0,
                render_image_back_image_view,
                sampler,
            )],
            [],
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

        let front_blit = FrontBlit::new(
            device.clone(),
            queue.clone(),
            render_image.clone(),
            memory_allocator.clone(),
        );

        let gpu_future = Some(image_clear_future.boxed());

        let command_builder = AutoCommandBufferBuilder::primary(
            command_buffer_allocator.clone(),
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
            self.memory_allocator.clone(),
            BufferCreateInfo {
                usage: BufferUsage::TRANSFER_SRC,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_HOST
                    | MemoryTypeFilter::HOST_SEQUENTIAL_WRITE,
                ..Default::default()
            },
            block.iter().cloned(),
        )
        .unwrap();

        let overflow_x = left + width > 1024;
        let overflow_y = top + height > 512;
        if overflow_x || overflow_y {
            let stage_image = Image::new(
                self.memory_allocator.clone(),
                ImageCreateInfo {
                    image_type: ImageType::Dim2d,
                    extent: [width, height, 1],
                    format: Format::A1R5G5B5_UNORM_PACK16,
                    usage: ImageUsage::TRANSFER_DST | ImageUsage::TRANSFER_SRC,
                    ..Default::default()
                },
                Default::default(),
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
                self.command_buffer_allocator.clone(),
                self.queue.queue_family_index(),
                CommandBufferUsage::OneTimeSubmit,
            )
            .unwrap();

        let buffer = Buffer::new_slice::<u16>(
            self.memory_allocator.clone(),
            BufferCreateInfo {
                usage: BufferUsage::TRANSFER_DST,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_HOST,
                ..Default::default()
            },
            (width * height) as u64,
        )
        .unwrap();

        let overflow_x = left + width > 1024;
        let overflow_y = top + height > 512;
        if overflow_x || overflow_y {
            let stage_image = Image::new(
                self.memory_allocator.clone(),
                ImageCreateInfo {
                    image_type: ImageType::Dim2d,
                    extent: [width, height, 1],
                    format: Format::A1R5G5B5_UNORM_PACK16,
                    usage: ImageUsage::TRANSFER_DST | ImageUsage::TRANSFER_SRC,
                    ..Default::default()
                },
                Default::default(),
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

    pub fn vram_vram_blit(
        &mut self,
        src_range: (Range<u32>, Range<u32>),
        dst_range: (Range<u32>, Range<u32>),
    ) {
        if src_range == dst_range {
            return;
        }
        // TODO: use vulkan image copy itself
        let block = self.read_vram_block(src_range);
        self.write_vram_block(dst_range, &block);
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
                Default::default(),
            )
            .unwrap()
            .clear_attachments(
                [ClearAttachment::Color {
                    color_attachment: 0,
                    clear_value: ClearColorValue::Float([
                        // switch the order of Red and Green, because our memory color ordering
                        // is swapped, and we swap it back on front_blit
                        color.2 as f32 / 255.0,
                        color.1 as f32 / 255.0,
                        color.0 as f32 / 255.0,
                        0.0,
                    ]),
                }]
                .into_iter()
                .collect(),
                [ClearRect {
                    offset: [top_left.0, top_left.1],
                    extent: [width, height],
                    array_layers: 0..1,
                }]
                .into_iter()
                .collect(),
            )
            .unwrap()
            .end_render_pass(Default::default())
            .unwrap();
        self.increment_command_builder_commands_and_flush();
    }

    /// Create ColorBlendState for a specific semi_transparency_mode, to be
    /// used to create a specific pipeline for it.
    fn create_color_blend_state(semi_transparency_mode: u8) -> ColorBlendState {
        // Mode 3 has no blend, so it is used for non_transparent draws
        let blend = match semi_transparency_mode {
            0 => Some(AttachmentBlend {
                // color_op: BlendOp::Add,
                // color_source: BlendFactor::SrcAlpha,
                // color_destination: BlendFactor::OneMinusSrcAlpha,
                // alpha_op: BlendOp::Add,
                // alpha_source: BlendFactor::One,
                // alpha_destination: BlendFactor::Zero,
                color_blend_op: BlendOp::Add,
                src_color_blend_factor: BlendFactor::SrcAlpha,
                dst_color_blend_factor: BlendFactor::OneMinusSrcAlpha,
                alpha_blend_op: BlendOp::Add,
                src_alpha_blend_factor: BlendFactor::One,
                dst_alpha_blend_factor: BlendFactor::Zero,
            }),
            1 => Some(AttachmentBlend {
                color_blend_op: BlendOp::Add,
                src_color_blend_factor: BlendFactor::One,
                dst_color_blend_factor: BlendFactor::SrcAlpha,
                alpha_blend_op: BlendOp::Add,
                src_alpha_blend_factor: BlendFactor::One,
                dst_alpha_blend_factor: BlendFactor::Zero,
            }),
            2 => Some(AttachmentBlend {
                color_blend_op: BlendOp::ReverseSubtract,
                src_color_blend_factor: BlendFactor::One,
                dst_color_blend_factor: BlendFactor::SrcAlpha,
                alpha_blend_op: BlendOp::Add,
                src_alpha_blend_factor: BlendFactor::One,
                dst_alpha_blend_factor: BlendFactor::Zero,
            }),
            3 => None,
            // NOTE: this is not a valid semi_transparency_mode, but we
            //       used it to create a faster path for non-textured mode 3
            //
            // faster path for mode 3 non-textured
            4 => Some(AttachmentBlend {
                color_blend_op: BlendOp::Add,
                src_color_blend_factor: BlendFactor::ConstantAlpha,
                dst_color_blend_factor: BlendFactor::One,
                alpha_blend_op: BlendOp::Add,
                src_alpha_blend_factor: BlendFactor::One,
                dst_alpha_blend_factor: BlendFactor::Zero,
            }),
            _ => unreachable!(),
        };
        ColorBlendState {
            logic_op: None,
            attachments: vec![ColorBlendAttachmentState {
                blend,
                color_write_mask: ColorComponents::R | ColorComponents::G | ColorComponents::B,
                color_write_enable: true,
            }],
            blend_constants: match semi_transparency_mode {
                4 => [0.0, 0.0, 0.0, 0.25],
                _ => [0.0, 0.0, 0.0, 0.0],
            },
            ..Default::default()
        }
    }

    fn new_command_buffer_builder(&mut self) -> AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> {
        AutoCommandBufferBuilder::primary(
            self.command_buffer_allocator.clone(),
            self.queue.queue_family_index(),
            CommandBufferUsage::OneTimeSubmit,
        )
        .unwrap()
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

        let mut future = self.gpu_future.take().unwrap();
        future.cleanup_finished();
        self.gpu_future = Some(
            future
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
            self.memory_allocator.clone(),
            BufferCreateInfo {
                usage: BufferUsage::VERTEX_BUFFER,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_HOST
                    | MemoryTypeFilter::HOST_SEQUENTIAL_WRITE,
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

        self.command_builder
            .begin_render_pass(
                RenderPassBeginInfo {
                    clear_values: vec![None],
                    ..RenderPassBeginInfo::framebuffer(self.render_image_framebuffer.clone())
                },
                Default::default(),
            )
            .unwrap()
            .set_viewport(
                0,
                [Viewport {
                    offset: [current_state.left as f32, current_state.top as f32],
                    extent: [current_state.width as f32, current_state.height as f32],
                    depth_range: 0.0..=1.0,
                }]
                .into_iter()
                .collect(),
            )
            .unwrap()
            .bind_descriptor_sets(
                PipelineBindPoint::Graphics,
                pipeline.layout().clone(),
                0,
                self.descriptor_set.clone(),
            )
            .unwrap()
            .bind_pipeline_graphics(pipeline.clone())
            .unwrap()
            .push_constants(pipeline.layout().clone(), 0, push_constants)
            .unwrap()
            .bind_vertex_buffers(0, vertex_buffer)
            .unwrap();
        // Safety: Shader safety, tested
        unsafe {
            self.command_builder
                .draw(vertices_len as u32, 1, 0, 0)
                .unwrap();
        }
        self.command_builder
            .end_render_pass(Default::default())
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
    #[allow(clippy::too_many_arguments)]
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
        let height = (drawing_bottom + 1).saturating_sub(drawing_top);
        let width = (drawing_right + 1).saturating_sub(drawing_left);

        if height == 0 || width == 0 {
            return;
        }

        let texture_window_mask = state_snapshot.texture_window_mask;
        let texture_window_offset = state_snapshot.texture_window_offset;

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

        let converted_vertices_iter = vertices.iter().map(|v| {
            DrawingVertexFull::new(
                v,
                &texture_params,
                texture_window_mask,
                texture_window_offset,
                semi_transparency_mode,
                semi_transparent,
                gpu_stat.dither_enabled(),
                textured,
                texture_blending,
            )
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

        let (mut topleft, size) = if full_vram {
            ([0; 2], [1024, 512])
        } else {
            // (((X2-X1)/cycles_per_pix)+2) AND NOT 3
            let mut horizontal_size = (((state_snapshot.display_horizontal_range.1
                - state_snapshot.display_horizontal_range.0)
                / gpu_stat.horizontal_dots_divider())
                + 2)
                & !3;

            if horizontal_size == 0 {
                horizontal_size = gpu_stat.horizontal_resolution();
            }

            let should_double = gpu_stat.vertical_resolution() == 480;

            // Y2-Y1, double if we are interlacing
            let mut vertical_size = (state_snapshot.display_vertical_range.1
                - state_snapshot.display_vertical_range.0)
                << should_double as u32;

            if vertical_size == 0 {
                vertical_size = gpu_stat.vertical_resolution();
            }

            (
                [vram_display_area_start.0, vram_display_area_start.1],
                [horizontal_size, vertical_size],
            )
        };

        // the rendering offset is more of a byte offset than pixel offset
        // so in 24bit mode, we have to change that.
        if gpu_stat.is_24bit_color_depth() {
            topleft[0] = (topleft[0] * 2) / 3;
        }

        let front_image = Image::new(
            self.memory_allocator.clone(),
            ImageCreateInfo {
                image_type: ImageType::Dim2d,
                extent: [size[0], size[1], 1],
                format: Format::B8G8R8A8_UNORM,
                usage: ImageUsage::TRANSFER_DST
                    | ImageUsage::TRANSFER_SRC
                    | ImageUsage::COLOR_ATTACHMENT,
                ..Default::default()
            },
            Default::default(),
        )
        .unwrap();

        // TODO: try to remove the `wait` from here
        self.front_blit
            .blit(
                front_image.clone(),
                topleft,
                size,
                !full_vram && gpu_stat.is_24bit_color_depth(),
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
