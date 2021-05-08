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

    drawing_area_top_left: (u32, u32),
    drawing_area_bottom_right: (u32, u32),
    drawing_offset: (i32, i32),
    texture_window_mask: (u32, u32),
    texture_window_offset: (u32, u32),
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
            0x00 | 0x06 => {
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
            0xe2 => {
                // Texture window settings
                let mask_x = data & 0x1F;
                let mask_y = (data >> 5) & 0x1F;
                let offset_x = (data >> 10) & 0x1F;
                let offset_y = (data >> 15) & 0x1F;

                self.texture_window_mask = (mask_x, mask_y);
                self.texture_window_offset = (offset_x, offset_y);
            }
            0xe3 => {
                // Set Drawing Area top left
                let x = data & 0x3ff;
                let y = (data >> 10) & 0x3ff;
                self.drawing_area_top_left = (x, y);
            }
            0xe4 => {
                // Set Drawing Area bottom right
                let x = data & 0x3ff;
                let y = (data >> 10) & 0x3ff;
                self.drawing_area_bottom_right = (x, y);
            }
            0xe5 => {
                // Set Drawing offset
                // TODO: test the accuracy of the sign extension
                let x = data & 0x7ff;
                let sign_extend = 0xfffff800 * ((x >> 10) & 1);
                let x = (x | sign_extend) as i32;
                let y = (data >> 11) & 0x7ff;
                let sign_extend = 0xfffff800 * ((y >> 10) & 1);
                let y = (y | sign_extend) as i32;
                self.drawing_offset = (x, y);
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
            0x04 => {
                // DMA direction
                self.gpu_stat.remove(GpuStat::DMA_DIRECTION);
                self.gpu_stat.bits |= (data & 3) << 29;

                // TODO: should also affect GpuStat::DMA_DATA_REQUEST
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
