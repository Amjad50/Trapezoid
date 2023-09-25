use std::sync::Arc;

use vulkano::{
    buffer::{Buffer, BufferContents, BufferCreateInfo, BufferUsage, Subbuffer},
    command_buffer::{
        allocator::StandardCommandBufferAllocator, AutoCommandBufferBuilder,
        CommandBufferExecFuture, CommandBufferUsage, CopyBufferToImageInfo, CopyImageToBufferInfo,
        PrimaryAutoCommandBuffer, RenderPassBeginInfo, SubpassContents,
    },
    descriptor_set::{
        allocator::StandardDescriptorSetAllocator, PersistentDescriptorSet, WriteDescriptorSet,
    },
    device::{Device, Queue},
    format::Format,
    image::{
        view::{ImageView, ImageViewCreateInfo},
        ImageAccess, ImageCreateFlags, ImageDimensions, ImageUsage, StorageImage,
    },
    memory::allocator::{AllocationCreateInfo, MemoryAllocator, MemoryUsage},
    pipeline::{
        graphics::{
            input_assembly::{InputAssemblyState, PrimitiveTopology},
            vertex_input::Vertex as VertexTrait,
            viewport::{Viewport, ViewportState},
        },
        ComputePipeline, GraphicsPipeline, Pipeline, PipelineBindPoint,
    },
    render_pass::{Framebuffer, FramebufferCreateInfo, RenderPass, Subpass},
    sampler::{
        ComponentMapping, ComponentSwizzle, Filter, Sampler, SamplerAddressMode, SamplerCreateInfo,
        SamplerMipmapMode,
    },
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

    command_buffer_allocator: StandardCommandBufferAllocator,
    descriptor_set_allocator: StandardDescriptorSetAllocator,

    texture_image: Arc<StorageImage>,

    texture_24bit_image: Arc<StorageImage>,
    texture_24bit_in_buffer: Subbuffer<[u16]>,
    texture_24bit_out_buffer: Subbuffer<[u32]>,
    texture_24bit_desc_set: Arc<PersistentDescriptorSet>,

    render_pass: Arc<RenderPass>,
    g_pipeline: Arc<GraphicsPipeline>,
    c_pipeline: Arc<ComputePipeline>,

    vertex_buffer: Subbuffer<[Vertex]>,
}

impl FrontBlit {
    pub fn new(
        device: Arc<Device>,
        queue: Arc<Queue>,
        source_image: Arc<StorageImage>,
        memory_allocator: &impl MemoryAllocator,
    ) -> Self {
        let vs = vs::load(device.clone()).unwrap();
        let fs = fs::load(device.clone()).unwrap();
        let cs = cs::load(device.clone()).unwrap();

        let descriptor_set_allocator = StandardDescriptorSetAllocator::new(device.clone());
        let command_buffer_allocator =
            StandardCommandBufferAllocator::new(device.clone(), Default::default());

        let render_pass = vulkano::single_pass_renderpass!(
            device.clone(),
            attachments: {
                color: {
                    load: DontCare,
                    store: Store,
                    format: Format::B8G8R8A8_UNORM,
                    samples: 1,
                }
            },
            pass: {
                color: [color],
                depth_stencil: {}
            }
        )
        .unwrap();

        let g_pipeline = GraphicsPipeline::start()
            .vertex_input_state(Vertex::per_vertex())
            .vertex_shader(vs.entry_point("main").unwrap(), ())
            .input_assembly_state(
                InputAssemblyState::new().topology(PrimitiveTopology::TriangleStrip),
            )
            .viewport_state(ViewportState::viewport_dynamic_scissor_irrelevant())
            .fragment_shader(fs.entry_point("main").unwrap(), ())
            .render_pass(Subpass::from(render_pass.clone(), 0).unwrap())
            .build(device.clone())
            .unwrap();

        let c_pipeline = ComputePipeline::new(
            device.clone(),
            cs.entry_point("main").unwrap(),
            &(),
            None,
            |_| {},
        )
        .unwrap();

        let texture_24bit_image = StorageImage::with_usage(
            memory_allocator,
            ImageDimensions::Dim2d {
                width: 1024,
                height: 512,
                array_layers: 1,
            },
            Format::B8G8R8A8_UNORM,
            ImageUsage::TRANSFER_DST | ImageUsage::SAMPLED,
            ImageCreateFlags::empty(),
            [queue.queue_family_index()],
        )
        .unwrap();

        let texture_24bit_in_buffer = Buffer::new_slice::<u16>(
            memory_allocator,
            BufferCreateInfo {
                usage: BufferUsage::TRANSFER_DST | BufferUsage::STORAGE_BUFFER,
                ..Default::default()
            },
            AllocationCreateInfo {
                usage: MemoryUsage::DeviceOnly,
                ..Default::default()
            },
            1024 * 512,
        )
        .unwrap();

        let texture_24bit_out_buffer = Buffer::new_slice::<u32>(
            memory_allocator,
            BufferCreateInfo {
                usage: BufferUsage::TRANSFER_SRC | BufferUsage::STORAGE_BUFFER,
                ..Default::default()
            },
            AllocationCreateInfo {
                usage: MemoryUsage::DeviceOnly,
                ..Default::default()
            },
            1024 * 512,
        )
        .unwrap();

        let texture_24bit_desc_set = PersistentDescriptorSet::new(
            &descriptor_set_allocator,
            c_pipeline.layout().set_layouts().get(0).unwrap().clone(),
            [
                WriteDescriptorSet::buffer(0, texture_24bit_in_buffer.clone()),
                WriteDescriptorSet::buffer(1, texture_24bit_out_buffer.clone()),
            ],
        )
        .unwrap();

        let vertex_buffer = Buffer::from_iter(
            memory_allocator,
            BufferCreateInfo {
                usage: BufferUsage::VERTEX_BUFFER,
                ..Default::default()
            },
            AllocationCreateInfo {
                usage: MemoryUsage::Upload,
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

    pub fn blit<D, IF>(
        &mut self,
        dest_image: Arc<D>,
        topleft: [u32; 2],
        size: [u32; 2],
        is_24bit_color_depth: bool,
        in_future: IF,
    ) -> CommandBufferExecFuture<IF>
    where
        D: ImageAccess + std::fmt::Debug + 'static,
        IF: GpuFuture,
    {
        let span = tracing::trace_span!("FrontBlit::blit");
        let _enter = span.enter();

        let [width, height] = dest_image.dimensions().width_height();

        let mut source_image = self.texture_image.clone();

        let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
            AutoCommandBufferBuilder::primary(
                &self.command_buffer_allocator,
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
                .bind_descriptor_sets(
                    PipelineBindPoint::Compute,
                    self.c_pipeline.layout().clone(),
                    0,
                    self.texture_24bit_desc_set.clone(),
                )
                .dispatch([
                    COMPUTE_24BIT_ROW_OPERATIONS / COMPUTE_LOCAL_SIZE_XY,
                    512 / COMPUTE_LOCAL_SIZE_XY,
                    1,
                ])
                .unwrap()
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

        let layout = self.g_pipeline.layout().set_layouts().get(0).unwrap();

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

        let set = PersistentDescriptorSet::new(
            &self.descriptor_set_allocator,
            layout.clone(),
            [WriteDescriptorSet::image_view_sampler(
                0,
                texture_image_view,
                sampler,
            )],
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
                SubpassContents::Inline,
            )
            .unwrap()
            .set_viewport(
                0,
                [Viewport {
                    origin: [0.0, 0.0],
                    dimensions: [width as f32, height as f32],
                    depth_range: 0.0..1.0,
                }],
            )
            .bind_pipeline_graphics(self.g_pipeline.clone())
            .bind_descriptor_sets(
                PipelineBindPoint::Graphics,
                self.g_pipeline.layout().clone(),
                0,
                set,
            )
            .push_constants(self.g_pipeline.layout().clone(), 0, push_constants)
            .bind_vertex_buffers(0, self.vertex_buffer.clone())
            .draw(4, 1, 0, 0)
            .unwrap()
            .end_render_pass()
            .unwrap();

        let command_buffer = builder.build().unwrap();

        in_future
            .then_execute(self.queue.clone(), command_buffer)
            .unwrap()
    }
}
