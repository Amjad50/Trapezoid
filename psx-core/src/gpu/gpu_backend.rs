use crate::gpu::{
    command::{instantiate_gp0_command, Gp0CmdType},
    GpuStat,
};

use super::{command::Gp0Command, gpu_context::GpuContext, BackendCommand};
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
    /// holds commands that needs extra parameter and complex, like sending
    /// to/from VRAM, and rendering
    current_command: Option<Box<dyn Gp0Command>>,

    gpu_backend_receiver: Receiver<BackendCommand>,
}

// for easier access to gpu context
impl std::ops::Deref for GpuBackend {
    type Target = GpuContext;

    fn deref(&self) -> &Self::Target {
        &self.gpu_context
    }
}

// for easier access to gpu context
impl std::ops::DerefMut for GpuBackend {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.gpu_context
    }
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
                    device.clone(),
                    queue.clone(),
                    gpu_stat,
                    gpu_read_sender,
                    gpu_front_image_sender,
                ),
                current_command: None,
                gpu_backend_receiver,
            };
            b.run();
        })
    }

    fn run(mut self) {
        loop {
            match self.gpu_backend_receiver.recv() {
                Ok(BackendCommand::Gp0Write(data)) => self.handle_gp0_input(data),
                Ok(BackendCommand::Gp1Write(data)) => self.run_gp1_command(data),
                Ok(BackendCommand::BlitFront(full_vram)) => {
                    self.blit_to_front(full_vram);
                }
                Err(_) => {}
            }
        }
    }
}

impl GpuBackend {
    fn handle_gp0_input(&mut self, data: u32) {
        log::info!("GPU: GP0 write: {:08x}", data);
        // if we still executing some command
        if let Some(cmd) = self.current_command.as_mut() {
            if cmd.still_need_params() {
                log::info!("gp0 extra param {:08X}", data);
                cmd.add_param(data);
                if !cmd.still_need_params() {
                    // take the self reference from here, so that we can update the gpu_stat
                    // without issues
                    let mut cmd = self.current_command.take().unwrap();

                    self.gpu_stat
                        .fetch_update(|s| Some(s - GpuStat::READY_FOR_DMA_RECV))
                        .unwrap();

                    log::info!("executing command {:?}", cmd.cmd_type());
                    cmd.exec_command(&mut self.gpu_context);
                    self.current_command = None;

                    // ready for next command
                    self.gpu_stat
                        .fetch_update(|s| {
                            Some(s | GpuStat::READY_FOR_CMD_RECV | GpuStat::READY_FOR_DMA_RECV)
                        })
                        .unwrap();
                }
            } else {
                unreachable!();
            }
        } else {
            let mut cmd = instantiate_gp0_command(data);
            log::info!("creating new command {:?}", cmd.cmd_type());
            if cmd.still_need_params() {
                self.current_command = Some(cmd);
                self.gpu_stat
                    .fetch_update(|s| Some(s - GpuStat::READY_FOR_CMD_RECV))
                    .unwrap();
            } else {
                cmd.exec_command(&mut self.gpu_context);
                self.current_command = None;
                log::info!("executing command {:?}", cmd.cmd_type());
            }
        }
    }

    fn run_gp1_command(&mut self, data: u32) {
        let cmd = data >> 24;
        log::info!("gp1 command {:02X} data: {:08X}", cmd, data);
        match cmd {
            0x00 => {
                // Reset Gpu
                // TODO: check what we need to do in reset
                self.write_gpu_stat(
                    GpuStat::DISPLAY_DISABLED
                        | GpuStat::INTERLACE_FIELD
                        | GpuStat::READY_FOR_DMA_RECV
                        | GpuStat::READY_FOR_CMD_RECV,
                );
            }
            0x01 => {
                // Reset command fifo buffer

                // TODO: reset the sender buffer
                if let Some(cmd) = &mut self.current_command {
                    if let Gp0CmdType::CpuToVramBlit = cmd.cmd_type() {
                        // flush vram write

                        // FIXME: close the write here and flush
                        //  do not add more data
                        //while !cmd.exec_command(&mut self.gpu_context) {
                        //    if cmd.still_need_params() {
                        //        cmd.add_param(0);
                        //    }
                        //}
                        self.current_command = None;
                        todo!();
                    }
                }
            }
            0x02 => {
                // Reset IRQ
                self.gpu_stat
                    .fetch_update(|s| Some(s.difference(GpuStat::INTERRUPT_REQUEST)))
                    .unwrap();
            }
            0x03 => {
                // Display enable
                self.gpu_stat
                    .fetch_update(|s| {
                        if data & 1 == 1 {
                            Some(s.union(GpuStat::DISPLAY_DISABLED))
                        } else {
                            Some(s.difference(GpuStat::DISPLAY_DISABLED))
                        }
                    })
                    .unwrap();
            }
            0x04 => {
                // DMA direction
                // TODO: should also affect GpuStat::DMA_DATA_REQUEST
                self.gpu_stat
                    .fetch_update(|mut s| {
                        s.remove(GpuStat::DMA_DIRECTION);
                        s.bits |= (data & 3) << 29;
                        Some(s)
                    })
                    .unwrap();
            }
            0x05 => {
                // Vram Start of Display area

                let x = data & 0x3ff;
                let y = (data >> 10) & 0x1ff;

                self.vram_display_area_start = (x, y);
                log::info!("vram display start area {:?}", self.vram_display_area_start);
            }
            0x06 => {
                // Screen Horizontal Display range
                let x1 = data & 0xfff;
                let x2 = (data >> 12) & 0xfff;

                self.display_horizontal_range = (x1, x2);
                log::info!(
                    "display horizontal range {:?}",
                    self.display_horizontal_range
                );
            }
            0x07 => {
                // Screen Vertical Display range
                let y1 = data & 0x1ff;
                let y2 = (data >> 10) & 0x1ff;

                self.display_vertical_range = (y1, y2);
                log::info!("display vertical range {:?}", self.display_vertical_range);
            }
            0x08 => {
                // Display mode

                // 17-18 Horizontal Resolution 1     (0=256, 1=320, 2=512, 3=640)
                // 19    Vertical Resolution         (0=240, 1=480, when Bit22=1)
                // 20    Video Mode                  (0=NTSC/60Hz, 1=PAL/50Hz)
                // 21    Display Area Color Depth    (0=15bit, 1=24bit)
                // 22    Vertical Interlace          (0=Off, 1=On)
                let stat_bits_17_22 = data & 0x3F;
                let stat_bit_16_horizontal_resolution_2 = (data >> 6) & 1;
                let stat_bit_14_reverse_flag = (data >> 7) & 1;
                // the inverse of the vertical interlace
                let interlace_field = ((data >> 5) & 1) ^ 1;

                self.gpu_stat
                    .fetch_update(|mut s| {
                        s.bits &= !0x7f6000;
                        s.bits |= stat_bits_17_22 << 17;
                        s.bits |= stat_bit_14_reverse_flag << 14;
                        s.bits |= stat_bit_16_horizontal_resolution_2 << 16;
                        s.bits |= interlace_field << 13;
                        Some(s)
                    })
                    .unwrap();
            }
            0x09 => {
                // Allow texture disable
                self.allow_texture_disable = data & 1 == 1;
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
                        self.cached_gp0_e2
                    }
                    3 => {
                        // Read Draw area top left GP0(E3h)
                        self.cached_gp0_e3
                    }
                    4 => {
                        // Read Draw area bottom right GP0(E4h)
                        self.cached_gp0_e4
                    }
                    5 => {
                        // Read Draw offset GP0(E5h)
                        self.cached_gp0_e5
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

                self.send_to_gpu_read(result);
            }
            _ => todo!("gp1 command {:02X}", cmd),
        }
    }
}
