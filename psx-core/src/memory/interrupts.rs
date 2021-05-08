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

    fn read_u16(&mut self, addr: u32) -> u16 {
        match addr {
            0 => self.stat,
            2 => 0,
            4 => self.mask,
            6 => 0,
            _ => unreachable!(),
        }
    }

    fn write_u16(&mut self, addr: u32, data: u16) {
        log::info!("write interrupts 16, regs {:X} = {:08X}", addr, data);
        match addr {
            0 => {
                self.stat = data;
            }
            2 => {}
            4 => {
                self.mask = data;
            }
            6 => {}
            _ => unreachable!(),
        }
    }

    fn read_u8(&mut self, _addr: u32) -> u8 {
        todo!()
    }

    fn write_u8(&mut self, _addr: u32, _data: u8) {
        todo!()
    }
}
