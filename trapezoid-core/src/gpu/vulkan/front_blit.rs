use std::sync::Arc;

use vulkano::{
    buffer::{Buffer, BufferContents, BufferCreateInfo, BufferUsage, Subbuffer},
    command_buffer::{
        allocator::StandardCommandBufferAllocator, AutoCommandBufferBuilder,
        CommandBufferExecFuture, CommandBufferUsage, CopyBufferToImageInfo, CopyImageToBufferInfo,
        PrimaryAutoCommandBuffer, RenderPassBeginInfo,
    },
    descriptor_set::{
        allocator::StandardDescriptorSetAllocator, DescriptorSet, WriteDescriptorSet,
    },
    device::{Device, Queue},
    format::Format,
    image::{
        sampler::{
            ComponentMapping, ComponentSwizzle, Filter, Sampler, SamplerAddressMode,
            SamplerCreateInfo, SamplerMipmapMode,
        },
        view::{ImageView, ImageViewCreateInfo},
        Image, ImageCreateInfo, ImageType, ImageUsage,
    },
    memory::allocator::{AllocationCreateInfo, MemoryAllocator, MemoryTypeFilter},
    pipeline::{
        compute::ComputePipelineCreateInfo,
        graphics::{
            color_blend::{ColorBlendAttachmentState, ColorBlendState},
            input_assembly::{InputAssemblyState, PrimitiveTopology},
            multisample::MultisampleState,
            rasterization::RasterizationState,
            vertex_input::{Vertex as VertexTrait, VertexDefinition},
            viewport::{Viewport, ViewportState},
            GraphicsPipelineCreateInfo,
        },
        layout::PipelineDescriptorSetLayoutCreateInfo,
        ComputePipeline, DynamicState, GraphicsPipeline, Pipeline, PipelineBindPoint,
        PipelineLayout, PipelineShaderStageCreateInfo,
    },
    render_pass::{Framebuffer, FramebufferCreateInfo, RenderPass, Subpass},
    sync::GpuFuture,
};

const COMPUTE_24BIT_ROW_OPERATIONS: u32 = 512 / 3;
const COMPUTE_LOCAL_SIZE_XY: u32 = 8;

mod vs {
    vulkano_shaders::shader! {
        ty: "vertex",
        src: "
#version 450

layout(location = 0) in vec2 position;
layout(location = 0) out vec2 tex_coords;

layout(push_constant) uniform PushConstantData {
    uvec2 topleft;
    uvec2 size;
} pc;

void main() {
    gl_Position = vec4(position, 0.0, 1.0);

    vec2 topleft = vec2(pc.topleft.x / 1024.0, pc.topleft.y / 512.0);
    vec2 size = vec2(pc.size.x / 1024.0, pc.size.y / 512.0);

    tex_coords = (position  + 1.0) / 2.0;
    tex_coords = tex_coords * size + topleft;
}",
    }
}

mod fs {
    vulkano_shaders::shader! {
        ty: "fragment",
        src: "
#version 450

layout(location = 0) in vec2 tex_coords;
layout(location = 0) out vec4 f_color;

layout(set = 0, binding = 0) uniform sampler2D tex;

void main() {
    f_color = texture(tex, tex_coords);
}"
    }
}

mod cs {
    vulkano_shaders::shader! {
        ty: "compute",
        src: "
#version 450

layout(local_size_x = 8, local_size_y = 8, local_size_z = 1) in;

uint IN_W = 512;
uint OUT_W = 1024;
uint MAX_IN_X = (512 * 4 / 3) - 1;

// perform a whole operation each time.
uint ROW_OPERATIONS = IN_W / 3;

layout(set = 0, binding = 0) readonly buffer InData {
    uint data[];
} inImageData;
layout(set = 0, binding = 1) writeonly buffer OutData {
    uint data[];
} outImageData;

void main() {
    uint x = gl_GlobalInvocationID.x;
    uint y = gl_GlobalInvocationID.y;

    if (x >= ROW_OPERATIONS) {
        return;
    }

    // convert every 3 words into 4 24bit pixels.
    uint in1 = inImageData.data[y * IN_W + x * 3 + 0];
    uint in2 = inImageData.data[y * IN_W + x * 3 + 1];
    uint in3 = inImageData.data[y * IN_W + x * 3 + 2];

    uint out1 = in1 & 0xFFFFFF;
    uint out2 = (in1 >> 24) | ((in2 & 0xFFFF) << 8);
    uint out3 = (in2 >> 16) | ((in3 & 0xFF) << 16);
    uint out4 = in3 >> 8;

    outImageData.data[y * OUT_W + x * 4 + 0] = out1;
    outImageData.data[y * OUT_W + x * 4 + 1] = out2;
    outImageData.data[y * OUT_W + x * 4 + 2] = out3;
    outImageData.data[y * OUT_W + x * 4 + 3] = out4;
}"
    }
}

#[derive(Default, Debug, Clone, Copy, VertexTrait, BufferContents)]
#[repr(C)]
struct Vertex {
    #[format(R32G32_SFLOAT)]
    position: [f32; 2],
}

pub(super) struct FrontBlit {
    device: Arc<Device>,
    queue: Arc<Queue>,

    command_buffer_allocator: Arc<StandardCommandBufferAllocator>,
    descriptor_set_allocator: Arc<StandardDescriptorSetAllocator>,

    texture_image: Arc<Image>,

    texture_24bit_image: Arc<Image>,
    texture_24bit_in_buffer: Subbuffer<[u16]>,
    texture_24bit_out_buffer: Subbuffer<[u32]>,
    texture_24bit_desc_set: Arc<DescriptorSet>,

    render_pass: Arc<RenderPass>,
    g_pipeline: Arc<GraphicsPipeline>,
    c_pipeline: Arc<ComputePipeline>,

    vertex_buffer: Subbuffer<[Vertex]>,
}

impl FrontBlit {
    pub fn new(
        device: Arc<Device>,
        queue: Arc<Queue>,
        source_image: Arc<Image>,
        memory_allocator: Arc<dyn MemoryAllocator>,
    ) -> Self {
        let vs = vs::load(device.clone())
            .unwrap()
            .entry_point("main")
            .unwrap();
        let fs = fs::load(device.clone())
            .unwrap()
            .entry_point("main")
            .unwrap();
        let cs = cs::load(device.clone())
            .unwrap()
            .entry_point("main")
            .unwrap();

        let descriptor_set_allocator = Arc::new(StandardDescriptorSetAllocator::new(
            device.clone(),
            Default::default(),
        ));
        let command_buffer_allocator = Arc::new(StandardCommandBufferAllocator::new(
            device.clone(),
            Default::default(),
        ));

        let render_pass = vulkano::single_pass_renderpass!(
            device.clone(),
            attachments: {
                color: {
                    format: Format::B8G8R8A8_UNORM,
                    samples: 1,
                    load_op: DontCare,
                    store_op: Store,
                },
            },
            pass: {
                color: [color],
                depth_stencil: {},
            },
        )
        .unwrap();

        let vertex_input_state = Vertex::per_vertex().definition(&vs).unwrap();
        let g_stages = [
            PipelineShaderStageCreateInfo::new(vs),
            PipelineShaderStageCreateInfo::new(fs),
        ];

        let g_layout = PipelineLayout::new(
            device.clone(),
            PipelineDescriptorSetLayoutCreateInfo::from_stages(&g_stages)
                .into_pipeline_layout_create_info(device.clone())
                .unwrap(),
        )
        .unwrap();

        let subpass = Subpass::from(render_pass.clone(), 0).unwrap();
        let g_pipeline = GraphicsPipeline::new(
            device.clone(),
            None,
            GraphicsPipelineCreateInfo {
                stages: g_stages.iter().cloned().collect(),
                vertex_input_state: Some(vertex_input_state),
                input_assembly_state: Some(InputAssemblyState {
                    topology: PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                }),
                rasterization_state: Some(RasterizationState::default()),
                multisample_state: Some(MultisampleState::default()),
                viewport_state: Some(ViewportState::default()),
                dynamic_state: [DynamicState::Viewport].into_iter().collect(),
                color_blend_state: Some(ColorBlendState::with_attachment_states(
                    1,
                    ColorBlendAttachmentState::default(),
                )),
                subpass: Some(subpass.into()),
                ..GraphicsPipelineCreateInfo::layout(g_layout)
            },
        )
        .unwrap();

        let c_stage = PipelineShaderStageCreateInfo::new(cs);
        let c_layout = PipelineLayout::new(
            device.clone(),
            PipelineDescriptorSetLayoutCreateInfo::from_stages([&c_stage])
                .into_pipeline_layout_create_info(device.clone())
                .unwrap(),
        )
        .unwrap();
        let c_pipeline = ComputePipeline::new(
            device.clone(),
            None,
            ComputePipelineCreateInfo::stage_layout(c_stage, c_layout),
        )
        .unwrap();

        let texture_24bit_image = Image::new(
            memory_allocator.clone(),
            ImageCreateInfo {
                image_type: ImageType::Dim2d,
                format: Format::B8G8R8A8_UNORM,
                usage: ImageUsage::TRANSFER_DST | ImageUsage::SAMPLED,
                extent: [1024, 512, 1],
                ..Default::default()
            },
            AllocationCreateInfo::default(),
        )
        .unwrap();

        let texture_24bit_in_buffer = Buffer::new_slice::<u16>(
            memory_allocator.clone(),
            BufferCreateInfo {
                usage: BufferUsage::TRANSFER_DST | BufferUsage::STORAGE_BUFFER,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_DEVICE,
                ..Default::default()
            },
            1024 * 512,
        )
        .unwrap();

        let texture_24bit_out_buffer = Buffer::new_slice::<u32>(
            memory_allocator.clone(),
            BufferCreateInfo {
                usage: BufferUsage::TRANSFER_SRC | BufferUsage::STORAGE_BUFFER,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_DEVICE,
                ..Default::default()
            },
            1024 * 512,
        )
        .unwrap();

        let texture_24bit_desc_set = DescriptorSet::new(
            descriptor_set_allocator.clone(),
            c_pipeline.layout().set_layouts().first().unwrap().clone(),
            [
                WriteDescriptorSet::buffer(0, texture_24bit_in_buffer.clone()),
                WriteDescriptorSet::buffer(1, texture_24bit_out_buffer.clone()),
            ],
            [],
        )
        .unwrap();

        let vertex_buffer = Buffer::from_iter(
            memory_allocator.clone(),
            BufferCreateInfo {
                usage: BufferUsage::VERTEX_BUFFER,
                ..Default::default()
            },
            AllocationCreateInfo {
                memory_type_filter: MemoryTypeFilter::PREFER_HOST
                    | MemoryTypeFilter::HOST_SEQUENTIAL_WRITE,
                ..Default::default()
            },
            [
                Vertex {
                    position: [-1.0, -1.0],
                },
                Vertex {
                    position: [-1.0, 1.0],
                },
                Vertex {
                    position: [1.0, -1.0],
                },
                Vertex {
                    position: [1.0, 1.0],
                },
            ],
        )
        .unwrap();

        Self {
            device,
            queue,
            command_buffer_allocator,
            descriptor_set_allocator,
            texture_image: source_image,
            texture_24bit_image,
            texture_24bit_in_buffer,
            texture_24bit_out_buffer,
            texture_24bit_desc_set,
            render_pass,
            g_pipeline,
            c_pipeline,
            vertex_buffer,
        }
    }

    pub fn blit<IF>(
        &mut self,
        dest_image: Arc<Image>,
        topleft: [u32; 2],
        size: [u32; 2],
        is_24bit_color_depth: bool,
        mut in_future: IF,
    ) -> CommandBufferExecFuture<IF>
    where
        IF: GpuFuture,
    {
        in_future.cleanup_finished();
        let [width, height, _] = dest_image.extent();

        let mut source_image = self.texture_image.clone();

        let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
            AutoCommandBufferBuilder::primary(
                self.command_buffer_allocator.clone(),
                self.queue.queue_family_index(),
                CommandBufferUsage::OneTimeSubmit,
            )
            .unwrap();

        if is_24bit_color_depth {
            builder
                .copy_image_to_buffer(CopyImageToBufferInfo::image_buffer(
                    self.texture_image.clone(),
                    self.texture_24bit_in_buffer.clone(),
                ))
                .unwrap()
                .bind_pipeline_compute(self.c_pipeline.clone())
                .unwrap()
                .bind_descriptor_sets(
                    PipelineBindPoint::Compute,
                    self.c_pipeline.layout().clone(),
                    0,
                    self.texture_24bit_desc_set.clone(),
                )
                .unwrap();
            // Safety: Shader safety, tested
            unsafe {
                builder
                    .dispatch([
                        COMPUTE_24BIT_ROW_OPERATIONS / COMPUTE_LOCAL_SIZE_XY,
                        512 / COMPUTE_LOCAL_SIZE_XY,
                        1,
                    ])
                    .unwrap()
            };
            builder
                .copy_buffer_to_image(CopyBufferToImageInfo::buffer_image(
                    self.texture_24bit_out_buffer.clone(),
                    self.texture_24bit_image.clone(),
                ))
                .unwrap();

            source_image = self.texture_24bit_image.clone();
        }

        let sampler = Sampler::new(
            self.device.clone(),
            SamplerCreateInfo {
                mag_filter: Filter::Nearest,
                min_filter: Filter::Nearest,
                mipmap_mode: SamplerMipmapMode::Nearest,
                address_mode: [SamplerAddressMode::Repeat; 3],
                ..Default::default()
            },
        )
        .unwrap();

        let layout = self.g_pipeline.layout().set_layouts().first().unwrap();

        let texture_image_view = ImageView::new(
            source_image.clone(),
            ImageViewCreateInfo {
                component_mapping: ComponentMapping {
                    r: ComponentSwizzle::Blue,
                    b: ComponentSwizzle::Red,
                    ..Default::default()
                },
                ..ImageViewCreateInfo::from_image(&source_image)
            },
        )
        .unwrap();

        let set = DescriptorSet::new(
            self.descriptor_set_allocator.clone(),
            layout.clone(),
            [WriteDescriptorSet::image_view_sampler(
                0,
                texture_image_view,
                sampler,
            )],
            [],
        )
        .unwrap();
        let framebuffer = Framebuffer::new(
            self.render_pass.clone(),
            FramebufferCreateInfo {
                attachments: vec![ImageView::new_default(dest_image).unwrap()],
                ..Default::default()
            },
        )
        .unwrap();

        let push_constants = vs::PushConstantData { topleft, size };

        builder
            .begin_render_pass(
                RenderPassBeginInfo {
                    clear_values: vec![None],
                    ..RenderPassBeginInfo::framebuffer(framebuffer)
                },
                Default::default(),
            )
            .unwrap()
            .set_viewport(
                0,
                [Viewport {
                    offset: [0.0, 0.0],
                    extent: [width as f32, height as f32],
                    depth_range: 0.0..=1.0,
                }]
                .into_iter()
                .collect(),
            )
            .unwrap()
            .bind_pipeline_graphics(self.g_pipeline.clone())
            .unwrap()
            .bind_descriptor_sets(
                PipelineBindPoint::Graphics,
                self.g_pipeline.layout().clone(),
                0,
                set.clone(),
            )
            .unwrap()
            .bind_descriptor_sets(
                PipelineBindPoint::Graphics,
                self.g_pipeline.layout().clone(),
                0,
                set.clone(),
            )
            .unwrap()
            .push_constants(self.g_pipeline.layout().clone(), 0, push_constants)
            .unwrap()
            .bind_vertex_buffers(0, self.vertex_buffer.clone())
            .unwrap();
        // Safety: Shader safety, tested
        unsafe { builder.draw(4, 1, 0, 0).unwrap() };
        builder.end_render_pass(Default::default()).unwrap();

        let command_buffer = builder.build().unwrap();

        in_future
            .then_execute(self.queue.clone(), command_buffer)
            .unwrap()
    }
}
