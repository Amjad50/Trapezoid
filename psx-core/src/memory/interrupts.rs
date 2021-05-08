use super::BusLine;

#[derive(Default)]
pub struct Interrupts {
    stat: u16,
    mask: u16,
}

impl BusLine for Interrupts {
    fn read_u32(&mut self, addr: u32) -> u32 {
        match addr {
            0 => self.stat as u32,
            4 => self.mask as u32,
            _ => unreachable!(),
        }
    }

    fn write_u32(&mut self, addr: u32, data: u32) {
        log::info!("write interrupts 32, regs {:X} = {:08X}", addr, data);
        match addr {
            0 => self.stat = data as u16,
            4 => self.mask = data as u16,
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
