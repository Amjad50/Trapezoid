use crate::gpu::GpuStat;

use super::{
    gpu_context::{GpuContext, GpuSharedState},
    BackendCommand,
};
use crossbeam::{
    atomic::AtomicCell,
    channel::{Receiver, Sender},
};
use std::{
    sync::{Arc, Mutex},
    thread::{self, JoinHandle},
};
use vulkano::{
    device::{Device, Queue},
    image::StorageImage,
};

pub struct GpuBackend {
    gpu_context: GpuContext,

    gpu_backend_receiver: Receiver<BackendCommand>,
}

impl GpuBackend {
    pub(super) fn start(
        device: Arc<Device>,
        queue: Arc<Queue>,
        gpu_stat: Arc<AtomicCell<GpuStat>>,
        shared_state: Arc<Mutex<GpuSharedState>>,
        gpu_read_sender: Sender<u32>,
        gpu_backend_receiver: Receiver<BackendCommand>,
        gpu_front_image_sender: Sender<Arc<StorageImage>>,
    ) -> JoinHandle<()> {
        thread::spawn(move || {
            let b = GpuBackend {
                gpu_context: GpuContext::new(
                    device,
                    queue,
                    gpu_stat,
                    shared_state,
                    gpu_read_sender,
                    gpu_front_image_sender,
                ),
                gpu_backend_receiver,
            };
            b.run();
        })
    }

    fn run(mut self) {
        loop {
            match self.gpu_backend_receiver.recv() {
                Ok(BackendCommand::BlitFront(full_vram)) => {
                    self.gpu_context.blit_to_front(full_vram);
                }
                Ok(BackendCommand::GpuCommand(mut command)) => {
                    assert!(!command.still_need_params());
                    command.exec_command(&mut self.gpu_context);
                }
                Err(_) => {}
            }
        }
    }
}
