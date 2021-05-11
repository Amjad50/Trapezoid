mod command;

use crate::memory::BusLine;

use command::{instantiate_gp0_command, Gp0Command};

bitflags::bitflags! {
    #[derive(Default)]
    struct GpuStat: u32 {
        const TEXTURE_PAGE_X_BASE      = 0b00000000000000000000000000001111;
        const TEXTURE_PAGE_Y_BASE      = 0b00000000000000000000000000010000;
        const SEMI_TRASPARENCY         = 0b00000000000000000000000001100000;
        const TEXTURE_PAGE_COLORS      = 0b00000000000000000000000110000000;
        const DITHER_ENABLED           = 0b00000000000000000000001000000000;
        const DRAWING_TO_DISPLAY_AREA  = 0b00000000000000000000010000000000;
        const DRAWING_MASK_BIT         = 0b00000000000000000000100000000000;
        const NO_DRAW_ON_MASK          = 0b00000000000000000001000000000000;
        const INTERLACE_FIELD          = 0b00000000000000000010000000000000;
        const REVERSE_FLAG             = 0b00000000000000000100000000000000;
        const DISABLE_TEXTURE          = 0b00000000000000001000000000000000;
        const HORIZONTAL_RESOLUTION2   = 0b00000000000000010000000000000000;
        const HORIZONTAL_RESOLUTION1   = 0b00000000000001100000000000000000;
        const VERTICAL_RESOLUTION      = 0b00000000000010000000000000000000;
        const VIDEO_MODE               = 0b00000000000100000000000000000000;
        const DISPLAY_AREA_COLOR_DEPTH = 0b00000000001000000000000000000000;
        const VERTICAL_INTERLACE       = 0b00000000010000000000000000000000;
        const DISPLAY_DISABLED         = 0b00000000100000000000000000000000;
        const INTERRUPT_REQUEST        = 0b00000001000000000000000000000000;
        const DMA_DATA_REQUEST         = 0b00000010000000000000000000000000;
        const READY_FOR_CMD_RECV       = 0b00000100000000000000000000000000;
        const READY_FOR_TO_SEND_VRAM   = 0b00001000000000000000000000000000;
        const READY_FOR_DMA_RECV       = 0b00010000000000000000000000000000;
        const DMA_DIRECTION            = 0b01100000000000000000000000000000;
        const INTERLACE_ODD_EVEN_LINES = 0b10000000000000000000000000000000;
    }
}

impl GpuStat {
    fn texture_page_coords(&self) -> (u32, u32) {
        let x = (self.bits & Self::TEXTURE_PAGE_X_BASE.bits) * 64;
        let y = (self.intersects(Self::TEXTURE_PAGE_Y_BASE) as u32) * 256;

        (x, y)
    }

    fn horizontal_resolution(&self) -> u32 {
        if self.intersects(Self::HORIZONTAL_RESOLUTION2) {
            368
        } else {
            // HORIZONTAL_RESOLUTION1 is two bits:
            // 0  (if set, Add 64 to the 256 original resoltion)
            // 1  (if set, Multiply the current resolution by 2)
            let resolution_multiplier = (self.bits & Self::HORIZONTAL_RESOLUTION1.bits) >> 17;
            let resoltion = 0x100 | ((resolution_multiplier & 1) << 14);
            resoltion << (resolution_multiplier >> 1)
        }
    }

    fn vertical_resolution(&self) -> u32 {
        240 * self.intersects(Self::VERTICAL_RESOLUTION) as u32
    }

    fn is_ntsc_video_mode(&self) -> bool {
        !self.intersects(Self::VIDEO_MODE)
    }

    fn display_enabled(&self) -> bool {
        !self.intersects(Self::DISPLAY_DISABLED)
    }
}

pub struct GpuContext {
    gpu_stat: GpuStat,

    drawing_area_top_left: (u32, u32),
    drawing_area_bottom_right: (u32, u32),
    drawing_offset: (i32, i32),
    texture_window_mask: (u32, u32),
    texture_window_offset: (u32, u32),

    vram_display_area_start: (u32, u32),
    display_horizontal_range: (u32, u32),
    display_vertical_range: (u32, u32),

    vram: Box<[u16; 1024 * 512]>,
}

impl Default for GpuContext {
    fn default() -> Self {
        Self {
            gpu_stat: Default::default(),
            drawing_area_top_left: Default::default(),
            drawing_area_bottom_right: Default::default(),
            drawing_offset: Default::default(),
            texture_window_mask: Default::default(),
            texture_window_offset: Default::default(),
            vram_display_area_start: Default::default(),
            display_horizontal_range: Default::default(),
            display_vertical_range: Default::default(),
            vram: Box::new([0; 1024 * 512]),
        }
    }
}

#[derive(Default)]
pub struct Gpu {
    gpu_context: GpuContext,
    /// holds commands that needs extra parameter and complex, like sending
    /// to/from VRAM, and rendering
    current_command: Option<Box<dyn Gp0Command>>,
}

// for easier access to gpu context
impl std::ops::Deref for Gpu {
    type Target = GpuContext;

    fn deref(&self) -> &Self::Target {
        &self.gpu_context
    }
}

// for easier access to gpu context
impl std::ops::DerefMut for Gpu {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.gpu_context
    }
}

impl Gpu {
    fn gpu_stat(&self) -> u32 {
        // Ready to receive Cmd Word
        // Ready to receive DMA Block
        let out = self.gpu_stat.bits | (0b101 << 26);

        log::info!("GPUSTAT = {:08X}", out);
        log::info!("GPUSTAT = {:?}", self.gpu_stat);
        out
    }

    fn gpu_read(&self) -> u32 {
        // TODO: get response from commands
        let out = 0;
        log::info!("GPUREAD = {:08X}", out);
        out
    }

    fn run_gp0_command(&mut self, data: u32) {
        // TODO: instead of executing the commands here, it should be done
        //  in a separate GPU clock, here should _only_ take input

        // if we still executing some command
        if let Some(cmd) = self.current_command.as_mut() {
            // add the new data we received
            log::info!("gp0 extra param {:08X}", data);
            cmd.add_param(data);
            // and exec, if it finished, then clear the current command
            if cmd.exec_command(&mut self.gpu_context) {
                self.current_command = None;
            }
            return;
        }

        log::info!("gp0 command {:08X}", data);
        let mut cmd = instantiate_gp0_command(data);

        // if its not finished yet
        if !cmd.exec_command(&mut self.gpu_context) {
            self.current_command = Some(cmd);
        }
    }

    fn run_gp1_command(&mut self, data: u32) {
        let cmd = data >> 24;
        log::info!("gp1 command {:02X} data: {:08X}", cmd, data);
        match cmd {
            0x00 => {
                // Reset Gpu
                self.gpu_stat = GpuStat::empty();
                self.gpu_stat.insert(
                    GpuStat::DISPLAY_DISABLED
                        | GpuStat::INTERLACE_FIELD
                        | GpuStat::READY_FOR_DMA_RECV
                        | GpuStat::READY_FOR_CMD_RECV,
                );
            }
            0x03 => {
                // Display enable
                self.gpu_stat.set(GpuStat::DISPLAY_DISABLED, data & 1 == 1)
            }
            0x04 => {
                // DMA direction
                self.gpu_stat.remove(GpuStat::DMA_DIRECTION);
                self.gpu_stat.bits |= (data & 3) << 29;

                // TODO: should also affect GpuStat::DMA_DATA_REQUEST
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

                self.gpu_stat.bits &= !0x7f6000;
                self.gpu_stat.bits |= stat_bits_17_22 << 17;
                self.gpu_stat.bits |= stat_bit_14_reverse_flag << 14;
                self.gpu_stat.bits |= stat_bit_16_horizontal_resolution_2 << 16;
                self.gpu_stat.bits |= interlace_field << 13;
            }
            _ => todo!("gp1 command {:02X}", cmd),
        }
    }
}

impl BusLine for Gpu {
    fn read_u32(&mut self, addr: u32) -> u32 {
        match addr {
            0 => self.gpu_read(),
            4 => self.gpu_stat(),
            _ => unreachable!(),
        }
    }

    fn write_u32(&mut self, addr: u32, data: u32) {
        match addr {
            0 => self.run_gp0_command(data),
            4 => self.run_gp1_command(data),
            _ => unreachable!(),
        }
    }

    fn read_u16(&mut self, _addr: u32) -> u16 {
        todo!()
    }

    fn write_u16(&mut self, _addr: u32, _data: u16) {
        todo!()
    }

    fn read_u8(&mut self, _addr: u32) -> u8 {
        todo!()
    }

    fn write_u8(&mut self, _addr: u32, _data: u8) {
        todo!()
    }
}
