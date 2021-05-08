use crate::memory::BusLine;

bitflags::bitflags! {
    #[derive(Default)]
    struct GpuStat: u32 {
        const TEXTURE_PAGE_X_BASE      = 0b00000000000000000000000000001111;
        const TEXTURE_PAGE_Y_BASE      = 0b00000000000000000000000000010000;
        const SEMI_TRASPARENCY         = 0b00000000000000000000000001100000;
        const TEXTURE_PAGE_COLORS      = 0b00000000000000000000000110000000;
        const DITHER_24_TO_15_BITS     = 0b00000000000000000000001000000000;
        const DRAWING_TO_DISPLAY_AREA  = 0b00000000000000000000010000000000;
        const DRAWING_MASK_BIT         = 0b00000000000000000000100000000000;
        const DRAW_PIXELS              = 0b00000000000000000001000000000000;
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

#[derive(Default)]
pub struct Gpu {
    gpu_stat: GpuStat,
}

impl Gpu {
    fn gpu_stat(&self) -> u32 {
        // Ready to receive Cmd Word
        // Ready to receive DMA Block
        let out = self.gpu_stat.bits | (0b101 << 26);

        log::info!("GPUSTAT = {:08X}", out);
        out
    }

    fn gpu_read(&self) -> u32 {
        // TODO: get response from commands
        let out = 0;
        log::info!("GPUREAD = {:08X}", out);
        out
    }
}

impl Gpu {
    fn run_gp0_command(&mut self, data: u32) {
        let cmd = data >> 24;
        log::info!("gp0 command {:02X} data: {:08X}", cmd, data);
        match cmd {
            0x00 => {
                // Nop
            }
            0xe1 => {
                // Draw Mode setting
                let stat_lower_11_bits = data & 0x7FF;
                let stat_15_bit = (data >> 11) & 1;

                #[allow(unused)]
                let textured_rect_x_flip = (data >> 12) & 1;
                #[allow(unused)]
                let textured_rect_y_flip = (data >> 13) & 1;

                self.gpu_stat.bits &= !0x87FF;
                self.gpu_stat.bits |= stat_lower_11_bits;
                self.gpu_stat.bits |= stat_15_bit << 15;
            }
            _ => todo!("gp0 command {:02X}", cmd),
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
            0x08 => {
                // Display mode
                let stat_bits_17_22 = data & 0x3F;
                let stat_bit_16 = (data >> 6) & 1;
                let stat_bit_14 = (data >> 7) & 1;
                // the inverse of the vertical interlace
                let interlace_field = ((data >> 5) & 1) ^ 1;

                self.gpu_stat.bits &= !0x7f6000;
                self.gpu_stat.bits |= stat_bits_17_22 << 17;
                self.gpu_stat.bits |= stat_bit_14 << 14;
                self.gpu_stat.bits |= stat_bit_16 << 16;
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
