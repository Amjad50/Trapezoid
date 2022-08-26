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
                gpu_context: GpuContext::new(
                    device,
                    queue,
                    gpu_stat,
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
                Ok(BackendCommand::Gp1Write(data)) => self.run_gp1_command(data),
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

impl GpuBackend {
    fn run_gp1_command(&mut self, data: u32) {
        let cmd = data >> 24;
        log::info!("gp1 command {:02X} data: {:08X}", cmd, data);
        match cmd {
            0x00 => {
                // handled by frontend
                unreachable!();
            }
            0x01 => {
                // handled by frontend
                unreachable!();
            }
            0x02 => {
                // handled by frontend
                unreachable!();
            }
            0x03 => {
                // handled by frontend
                unreachable!();
            }
            0x04 => {
                // handled by frontend
                unreachable!();
            }
            0x05 => {
                // Vram Start of Display area

                let x = data & 0x3ff;
                let y = (data >> 10) & 0x1ff;

                self.gpu_context.vram_display_area_start = (x, y);
                log::info!(
                    "vram display start area {:?}",
                    self.gpu_context.vram_display_area_start
                );
            }
            0x06 => {
                // Screen Horizontal Display range
                let x1 = data & 0xfff;
                let x2 = (data >> 12) & 0xfff;

                self.gpu_context.display_horizontal_range = (x1, x2);
                log::info!(
                    "display horizontal range {:?}",
                    self.gpu_context.display_horizontal_range
                );
            }
            0x07 => {
                // Screen Vertical Display range
                let y1 = data & 0x1ff;
                let y2 = (data >> 10) & 0x1ff;

                self.gpu_context.display_vertical_range = (y1, y2);
                log::info!(
                    "display vertical range {:?}",
                    self.gpu_context.display_vertical_range
                );
            }
            0x08 => {
                // handled by frontend
                unreachable!();
            }
            0x09 => {
                // Allow texture disable
                self.gpu_context.allow_texture_disable = data & 1 == 1;
            }
            0x10 => {
                // GPU info

                // 0x0~0xF retreive info, and the rest are mirrors
                let info_id = data & 0xF;

                // TODO: we don't need to be empty in our design, but we
                //       need the old data in some commands, so for now,
                //       lets make sure we don't have old data, until we
                //       store it somewhere.
                //assert!(self.gpu_read_sender.is_empty());

                // TODO: some commands read old value of GPUREAD, we can't do that
                // now. might need to change how we handle GPUREAD in general
                let result = match info_id {
                    2 => {
                        // Read Texture Window setting GP0(E2h)
                        self.gpu_context.cached_gp0_e2
                    }
                    3 => {
                        // Read Draw area top left GP0(E3h)
                        self.gpu_context.cached_gp0_e3
                    }
                    4 => {
                        // Read Draw area bottom right GP0(E4h)
                        self.gpu_context.cached_gp0_e4
                    }
                    5 => {
                        // Read Draw offset GP0(E5h)
                        self.gpu_context.cached_gp0_e5
                    }
                    6 => {
                        // return old value of GPUREAD
                        0
                    }
                    7 => {
                        // GPU type
                        2
                    }
                    8 => {
                        // unknown
                        0
                    }
                    _ => {
                        // return old value of GPUREAD
                        0
                    }
                };

                self.gpu_context.send_to_gpu_read(result);
            }
            _ => todo!("gp1 command {:02X}", cmd),
        }
    }
}
