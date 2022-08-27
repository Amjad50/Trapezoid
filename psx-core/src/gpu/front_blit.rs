use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use vulkano::{
    buffer::{BufferUsage, DeviceLocalBuffer, ImmutableBuffer},
    command_buffer::{
        AutoCommandBufferBuilder, CommandBufferExecFuture, CommandBufferUsage,
        PrimaryAutoCommandBuffer, SubpassContents,
    },
    descriptor_set::{PersistentDescriptorSet, WriteDescriptorSet},
    device::{Device, Queue},
    format::{ClearValue, Format},
    image::{
        view::{ImageView, ImageViewCreateInfo},
        ImageAccess, ImageCreateFlags, ImageDimensions, ImageUsage, StorageImage,
    },
    pipeline::{
        graphics::{
            input_assembly::{InputAssemblyState, PrimitiveTopology},
            vertex_input::BuffersDefinition,
            viewport::{Viewport, ViewportState},
        },
        ComputePipeline, GraphicsPipeline, Pipeline, PipelineBindPoint,
    },
    render_pass::{Framebuffer, FramebufferCreateInfo, RenderPass, Subpass},
    sampler::{
        ComponentMapping, ComponentSwizzle, Filter, Sampler, SamplerAddressMode, SamplerCreateInfo,
        SamplerMipmapMode,
    },
    sync::{GpuFuture, NowFuture},
};

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
}"
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

layout(set = 0, binding = 0) readonly buffer InData {
    uint data[];
} inImageData;
layout(set = 0, binding = 1) writeonly buffer OutData {
    uint data[];
} outImageData;

void main() {
    uint x = gl_GlobalInvocationID.x;
    uint y = gl_GlobalInvocationID.y;

    if (x > MAX_IN_X) {
        return;
    }

    uint result_data = 0;

    uint outBaseX = (x / 4) * 3;
    uint type = x % 4;

    if (type != 0) {
        outBaseX += (type - 1);
        uint d1 = inImageData.data[y * IN_W + outBaseX];

        if (type == 3) {
            result_data = d1 >> 8;
        } else {
            uint d2 = inImageData.data[y * IN_W + outBaseX + 1];

            if (type == 1) {
                result_data = (d1 >> 24) | ((d2 & 0xFFFF) << 8);
            } else {
                result_data = ((d1 >> 16) & 0xFFFF) | ((d2 & 0xFF) << 16);
            }
        }
    } else {
        uint d = inImageData.data[y * IN_W + outBaseX];
        result_data = d;
    }

    outImageData.data[y * OUT_W + x] = result_data & 0x00FFFFFF;
}"
    }
}

#[derive(Default, Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
struct Vertex {
    position: [f32; 2],
}

vulkano::impl_vertex!(Vertex, position);

pub(super) struct FrontBlit {
    device: Arc<Device>,
    queue: Arc<Queue>,

    texture_image: Arc<StorageImage>,

    texture_24bit_image: Arc<StorageImage>,
    texture_24bit_in_buffer: Arc<DeviceLocalBuffer<[u16]>>,
    texture_24bit_out_buffer: Arc<DeviceLocalBuffer<[u32]>>,
    texture_24bit_desc_set: Arc<PersistentDescriptorSet>,

    render_pass: Arc<RenderPass>,
    g_pipeline: Arc<GraphicsPipeline>,
    c_pipeline: Arc<ComputePipeline>,

    vertex_buffer: Arc<ImmutableBuffer<[Vertex]>>,
}

impl FrontBlit {
    pub fn new(
        device: Arc<Device>,
        queue: Arc<Queue>,
        source_image: Arc<StorageImage>,
    ) -> (
        Self,
        CommandBufferExecFuture<NowFuture, PrimaryAutoCommandBuffer>,
    ) {
        let vs = vs::load(device.clone()).unwrap();
        let fs = fs::load(device.clone()).unwrap();
        let cs = cs::load(device.clone()).unwrap();

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
            .vertex_input_state(BuffersDefinition::new().vertex::<Vertex>())
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
            device.clone(),
            ImageDimensions::Dim2d {
                width: 1024,
                height: 512,
                array_layers: 1,
            },
            Format::B8G8R8A8_UNORM,
            ImageUsage {
                transfer_destination: true,
                sampled: true,
                ..ImageUsage::none()
            },
            ImageCreateFlags::none(),
            Some(queue.family()),
        )
        .unwrap();

        let texture_24bit_in_buffer = DeviceLocalBuffer::<[u16]>::array(
            device.clone(),
            1024 * 512,
            BufferUsage {
                transfer_destination: true,
                storage_buffer: true,
                ..BufferUsage::none()
            },
            [queue.family()],
        )
        .unwrap();

        let texture_24bit_out_buffer = DeviceLocalBuffer::<[u32]>::array(
            device.clone(),
            1024 * 512,
            BufferUsage {
                transfer_source: true,
                storage_buffer: true,
                ..BufferUsage::none()
            },
            [queue.family()],
        )
        .unwrap();

        let texture_24bit_desc_set = PersistentDescriptorSet::new(
            c_pipeline.layout().set_layouts().get(0).unwrap().clone(),
            [
                WriteDescriptorSet::buffer(0, texture_24bit_in_buffer.clone()),
                WriteDescriptorSet::buffer(1, texture_24bit_out_buffer.clone()),
            ],
        )
        .unwrap();

        let (vertex_buffer, immutable_buffer_future) = ImmutableBuffer::from_iter(
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
            BufferUsage::all(),
            queue.clone(),
        )
        .unwrap();

        (
            Self {
                device,
                queue,
                texture_image: source_image,
                texture_24bit_image,
                texture_24bit_in_buffer,
                texture_24bit_out_buffer,
                texture_24bit_desc_set,
                render_pass,
                g_pipeline,
                c_pipeline,
                vertex_buffer,
            },
            immutable_buffer_future,
        )
    }

    pub fn blit<D, IF>(
        &mut self,
        dest_image: Arc<D>,
        topleft: [u32; 2],
        size: [u32; 2],
        is_24bit_color_depth: bool,
        in_future: IF,
    ) -> CommandBufferExecFuture<IF, PrimaryAutoCommandBuffer>
    where
        D: ImageAccess + 'static,
        IF: GpuFuture,
    {
        let [width, height] = dest_image.dimensions().width_height();

        let mut source_image = self.texture_image.clone();

        let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
            AutoCommandBufferBuilder::primary(
                self.device.clone(),
                self.queue.family(),
                CommandBufferUsage::OneTimeSubmit,
            )
            .unwrap();

        if is_24bit_color_depth {
            builder
                .copy_image_to_buffer(
                    self.texture_image.clone(),
                    self.texture_24bit_in_buffer.clone(),
                )
                .unwrap()
                .bind_pipeline_compute(self.c_pipeline.clone())
                .bind_descriptor_sets(
                    PipelineBindPoint::Compute,
                    self.c_pipeline.layout().clone(),
                    0,
                    self.texture_24bit_desc_set.clone(),
                )
                .dispatch([(512 * 4 / 3) / 8 + 1, 512 / 8, 1])
                .unwrap()
                .copy_buffer_to_image(
                    self.texture_24bit_out_buffer.clone(),
                    self.texture_24bit_image.clone(),
                )
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

        let push_constants = vs::ty::PushConstantData { topleft, size };

        builder
            .begin_render_pass(framebuffer, SubpassContents::Inline, [ClearValue::None])
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
