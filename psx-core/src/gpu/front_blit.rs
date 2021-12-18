use std::sync::Arc;

use vulkano::{
    buffer::{BufferUsage, ImmutableBuffer},
    command_buffer::{
        AutoCommandBufferBuilder, CommandBufferExecFuture, CommandBufferUsage,
        PrimaryAutoCommandBuffer, SubpassContents,
    },
    descriptor_set::PersistentDescriptorSet,
    device::{Device, Queue},
    format::{ClearValue, Format},
    image::{
        view::{ComponentMapping, ComponentSwizzle, ImageView},
        ImageAccess, StorageImage,
    },
    pipeline::{
        graphics::{
            input_assembly::{InputAssemblyState, PrimitiveTopology},
            vertex_input::BuffersDefinition,
            viewport::{Viewport, ViewportState},
        },
        GraphicsPipeline, Pipeline, PipelineBindPoint,
    },
    render_pass::{Framebuffer, RenderPass, Subpass},
    sampler::{Filter, MipmapMode, Sampler, SamplerAddressMode},
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

#[derive(Default, Debug, Clone)]
struct Vertex {
    position: [f32; 2],
}

vulkano::impl_vertex!(Vertex, position);

pub(super) struct FrontBlit {
    device: Arc<Device>,
    queue: Arc<Queue>,

    texture_image: Arc<StorageImage>,

    render_pass: Arc<RenderPass>,
    pipeline: Arc<GraphicsPipeline>,

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

        let pipeline = GraphicsPipeline::start()
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
                render_pass,
                pipeline,
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
        in_future: IF,
    ) -> CommandBufferExecFuture<IF, PrimaryAutoCommandBuffer>
    where
        D: ImageAccess + 'static,
        IF: GpuFuture,
    {
        let [width, height] = dest_image.dimensions().width_height();

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

        let layout = self
            .pipeline
            .layout()
            .descriptor_set_layouts()
            .get(0)
            .unwrap();
        let mut set_builder = PersistentDescriptorSet::start(layout.clone());

        let component_mapping = ComponentMapping {
            r: ComponentSwizzle::Blue,
            b: ComponentSwizzle::Red,
            ..Default::default()
        };
        set_builder
            .add_sampled_image(
                ImageView::start(self.texture_image.clone())
                    .with_component_mapping(component_mapping)
                    .build()
                    .unwrap(),
                sampler,
            )
            .unwrap();

        let set = set_builder.build().unwrap();

        let framebuffer = Framebuffer::start(self.render_pass.clone())
            .add(ImageView::new(dest_image).unwrap())
            .unwrap()
            .build()
            .unwrap();

        let push_constants = vs::ty::PushConstantData { topleft, size };

        let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
            AutoCommandBufferBuilder::primary(
                self.device.clone(),
                self.queue.family(),
                CommandBufferUsage::OneTimeSubmit,
            )
            .unwrap();

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
            .bind_pipeline_graphics(self.pipeline.clone())
            .bind_descriptor_sets(
                PipelineBindPoint::Graphics,
                self.pipeline.layout().clone(),
                0,
                set,
            )
            .push_constants(self.pipeline.layout().clone(), 0, push_constants)
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
