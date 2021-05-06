use super::BusLine;

pub struct Dma {
    control: u32,
}

impl Default for Dma {
    fn default() -> Self {
        Self {
            control: 0x07654321,
        }
    }
}

impl BusLine for Dma {
    fn read_u32(&mut self, addr: u32) -> u32 {
        match addr & 0xF0 {
            0x80 => todo!(),
            0x90 => todo!(),
            0xA0 => todo!(),
            0xB0 => todo!(),
            0xC0 => todo!(),
            0xD0 => todo!(),
            0xE0 => todo!(),
            0xF0 if addr == 0xF0 => self.control,
            0xF0 if addr == 0xF4 => todo!(),
            _ => unreachable!(),
        }
    }

    fn write_u32(&mut self, addr: u32, data: u32) {
        match addr & 0xF0 {
            0x80 => todo!(),
            0x90 => todo!(),
            0xA0 => todo!(),
            0xB0 => todo!(),
            0xC0 => todo!(),
            0xD0 => todo!(),
            0xE0 => todo!(),
            0xF0 if addr == 0xF0 => {
                println!("DMA control {:08X}", data);
                self.control = data
            }
            0xF0 if addr == 0xF4 => todo!(),
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
