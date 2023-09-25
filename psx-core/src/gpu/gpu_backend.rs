use crate::gpu::GpuStat;

use super::{gpu_context::GpuContext, BackendCommand};
use crossbeam::{
    atomic::AtomicCell,
    channel::{Receiver, Sender},
};
use std::{
    sync::Arc,
    thread::{self, JoinHandle},
};
use vulkano::{
    device::{Device, Queue},
    image::StorageImage,
};

pub struct GpuBackend {
    gpu_context: GpuContext,
    gpu_stat: Arc<AtomicCell<GpuStat>>,

    gpu_read_sender: Sender<u32>,
    gpu_backend_receiver: Receiver<BackendCommand>,
}

impl GpuBackend {
    pub(super) fn start(
        device: Arc<Device>,
        queue: Arc<Queue>,
        gpu_stat: Arc<AtomicCell<GpuStat>>,
        gpu_read_sender: Sender<u32>,
        gpu_backend_receiver: Receiver<BackendCommand>,
        gpu_front_image_sender: Sender<Arc<StorageImage>>,
    ) -> JoinHandle<()> {
        thread::spawn(move || {
            let b = GpuBackend {
                gpu_context: GpuContext::new(device, queue, gpu_front_image_sender),
                gpu_stat,
                gpu_read_sender,
                gpu_backend_receiver,
            };
            b.run();
        })
    }

    fn run(mut self) {
        loop {
            let cmd = self.gpu_backend_receiver.recv();

            let cmd_name = match cmd {
                Ok(BackendCommand::BlitFront { .. }) => "BlitFront",
                Ok(BackendCommand::DrawPolyline { .. }) => "DrawPolyline",
                Ok(BackendCommand::DrawPolygon { .. }) => "DrawPolygon",
                Ok(BackendCommand::WriteVramBlock { .. }) => "WriteVramBlock",
                Ok(BackendCommand::VramVramBlit { .. }) => "VramVramBlit",
                Ok(BackendCommand::VramReadBlock { .. }) => "VramReadBlock",
                Ok(BackendCommand::FillColor { .. }) => "FillColor",
                Err(_) => "Err",
            };
            let span = tracing::trace_span!("GpuBackend::run", arg = cmd_name);
            let _enter = span.enter();

            match cmd {
                Ok(BackendCommand::BlitFront {
                    full_vram,
                    state_snapshot,
                }) => {
                    self.gpu_context.blit_to_front(full_vram, state_snapshot);
                }
                Ok(BackendCommand::DrawPolyline {
                    vertices,
                    semi_transparent,
                    state_snapshot,
                }) => {
                    self.gpu_context
                        .draw_polyline(&vertices, semi_transparent, state_snapshot);
                }
                Ok(BackendCommand::DrawPolygon {
                    vertices,
                    texture_params,
                    textured,
                    texture_blending,
                    semi_transparent,
                    state_snapshot,
                }) => {
                    self.gpu_context.draw_polygon(
                        &vertices,
                        texture_params,
                        textured,
                        texture_blending,
                        semi_transparent,
                        state_snapshot,
                    );
                }
                Ok(BackendCommand::WriteVramBlock { block_range, block }) => {
                    self.gpu_context.write_vram_block(block_range, &block);
                }
                Ok(BackendCommand::VramVramBlit { src, dst }) => {
                    self.gpu_context.vram_vram_blit(src, dst);
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
                        self.gpu_read_sender.send(data).unwrap();
                    }
                    // after sending all the data, we set the gpu_stat bit to indicate that
                    // the data can be read now
                    self.gpu_stat
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
