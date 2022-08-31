use std::sync::Arc;

use crossbeam::channel::{Receiver, Sender};
use vulkano::{
    command_buffer::{AutoCommandBufferBuilder, CommandBufferUsage, PrimaryAutoCommandBuffer},
    device::{Device, Queue},
    image::{ImageAccess, StorageImage},
    sampler::Filter,
    sync::GpuFuture,
};

use super::BackendCommand;

pub struct GpuRenderer {
    device: Arc<Device>,
    queue: Arc<Queue>,

    // backend commands channel
    gpu_backend_sender: Sender<BackendCommand>,
    // channel for front image coming from backend
    gpu_front_image_receiver: Receiver<Arc<StorageImage>>,

    first_frame: bool,
    current_front_image: Option<Arc<StorageImage>>,
}

impl GpuRenderer {
    pub(super) fn new(
        device: Arc<Device>,
        queue: Arc<Queue>,
        gpu_backend_sender: Sender<BackendCommand>,
        gpu_front_image_receiver: Receiver<Arc<StorageImage>>,
    ) -> Self {
        Self {
            device,
            queue,

            gpu_backend_sender,
            gpu_front_image_receiver,

            first_frame: true,
            current_front_image: None,
        }
    }
}

impl GpuRenderer {
    pub fn sync_gpu_and_blit_to_front<D, IF>(
        &mut self,
        dest_image: Arc<D>,
        full_vram: bool,
        in_future: IF,
    ) where
        D: ImageAccess + 'static,
        IF: GpuFuture,
    {
        // if we have a previous image, then we are not in the first frame,
        // so there should be an image in the channel.
        if !self.first_frame {
            // `recv` is blocking, here we will wait for the GPU to finish all drawing.
            // FIXME: Do not block. Find a way to keep the GPU synced with minimal performance loss.
            self.current_front_image = Some(self.gpu_front_image_receiver.recv().unwrap());
        }
        self.first_frame = false;

        // send command for next frame from now, so when we recv later, its mostly will be ready
        self.gpu_backend_sender
            .send(BackendCommand::BlitFront(full_vram))
            .unwrap();

        if let Some(img) = self.current_front_image.as_ref() {
            let mut builder: AutoCommandBufferBuilder<PrimaryAutoCommandBuffer> =
                AutoCommandBufferBuilder::primary(
                    self.device.clone(),
                    self.queue.family(),
                    CommandBufferUsage::OneTimeSubmit,
                )
                .unwrap();

            builder
                .blit_image(
                    img.clone(),
                    [0, 0, 0],
                    [
                        img.dimensions().width() as i32,
                        img.dimensions().height() as i32,
                        1,
                    ],
                    0,
                    0,
                    dest_image.clone(),
                    [0, 0, 0],
                    [
                        dest_image.dimensions().width() as i32,
                        dest_image.dimensions().height() as i32,
                        1,
                    ],
                    0,
                    0,
                    1,
                    Filter::Nearest,
                )
                .unwrap();
            let cb = builder.build().unwrap();

            // TODO: remove wait
            in_future
                .then_execute(self.queue.clone(), cb)
                .unwrap()
                .then_signal_fence_and_flush()
                .unwrap()
                .wait(None)
                .unwrap();
        } else {
            // we must flush the future even if we are not using it.
            in_future.flush().unwrap();
        }
    }
}
