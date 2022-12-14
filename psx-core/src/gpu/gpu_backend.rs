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
                Ok(BackendCommand::BlitFront { full_vram }) => {
                    self.gpu_context.blit_to_front(full_vram);
                }
                Ok(BackendCommand::DrawPolyline {
                    vertices,
                    semi_transparent,
                }) => {
                    self.gpu_context.draw_polyline(&vertices, semi_transparent);
                }
                Ok(BackendCommand::DrawPolygon {
                    vertices,
                    texture_params,
                    textured,
                    texture_blending,
                    semi_transparent,
                }) => {
                    self.gpu_context.draw_polygon(
                        &vertices,
                        texture_params,
                        textured,
                        texture_blending,
                        semi_transparent,
                    );
                }
                Ok(BackendCommand::WriteVramBlock { block_range, block }) => {
                    self.gpu_context.write_vram_block(block_range, &block);
                }
                Ok(BackendCommand::VramVramBlit { src, dst }) => {
                    // TODO: use vulkan image copy itself
                    let block = self.gpu_context.read_vram_block(src);
                    self.gpu_context.write_vram_block(dst, &block);
                }
                Ok(BackendCommand::VramReadBlock { block_range }) => {
                    let src = (block_range.0.start, block_range.1.start);
                    let size = (
                        block_range.0.end - block_range.0.start,
                        block_range.1.end - block_range.1.start,
                    );

                    let block = self.gpu_context.read_vram_block(block_range);

                    let mut block_counter = 0;
                    while block_counter < block.len() {
                        // used for debugging only
                        let vram_pos = (
                            (block_counter as u32 % size.0) + src.0,
                            (block_counter as u32 / size.0) + src.1,
                        );
                        let d1 = block[block_counter];
                        let d2 = if block_counter + 1 < block.len() {
                            block[block_counter + 1]
                        } else {
                            0
                        };
                        block_counter += 2;

                        let data = ((d2 as u32) << 16) | d1 as u32;
                        log::info!("IN TRANSFERE, src={:?}, data={:08X}", vram_pos, data);

                        // TODO: send full block
                        self.gpu_context.send_to_gpu_read(data);
                    }
                    // after sending all the data, we set the gpu_stat bit to indicate that
                    // the data can be read now
                    self.gpu_context
                        .gpu_stat
                        .fetch_update(|s| Some(s | GpuStat::READY_FOR_TO_SEND_VRAM))
                        .unwrap();
                    log::info!("DONE TRANSFERE");
                }
                Ok(BackendCommand::FillColor {
                    top_left,
                    size,
                    color,
                }) => {
                    self.gpu_context.fill_color(top_left, size, color);
                }
                Err(_) => {}
            }
        }
    }
}
