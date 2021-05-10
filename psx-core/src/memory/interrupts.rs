use super::BusLine;

bitflags::bitflags! {
    #[derive(Default)]
    struct InterruptRegister: u16 {
        const VBLANK                 = 1 << 0;
        const GPU                    = 1 << 1;
        const CDROM                  = 1 << 2;
        const DMA                    = 1 << 3;
        const TIMER0                 = 1 << 4;
        const TIMER1                 = 1 << 5;
        const TIMER2                 = 1 << 6;
        const CONTROLLER_AND_MEMCARD = 1 << 7;
        const SIO                    = 1 << 8;
        const SPU                    = 1 << 9;
        const CONTROLLER             = 1 << 10;
    }
}

#[derive(Default)]
pub struct Interrupts {
    stat: InterruptRegister,
    mask: InterruptRegister,
}

impl BusLine for Interrupts {
    fn read_u32(&mut self, addr: u32) -> u32 {
        match addr {
            0 => self.stat.bits as u32,
            4 => self.mask.bits as u32,
            _ => unreachable!(),
        }
    }

    fn write_u32(&mut self, addr: u32, data: u32) {
        log::info!("write interrupts 32, regs {:X} = {:08X}", addr, data);
        match addr {
            0 => {
                self.stat = InterruptRegister::from_bits_truncate(data as u16);
                log::info!("write interrupts stat {:?}", self.stat);
            }
            4 => {
                self.mask = InterruptRegister::from_bits_truncate(data as u16);
                log::info!("write interrupts mask {:?}", self.mask);
            }
            _ => unreachable!(),
        }
    }

    fn read_u16(&mut self, addr: u32) -> u16 {
        match addr {
            0 => self.stat.bits,
            2 => 0,
            4 => self.mask.bits,
            6 => 0,
            _ => unreachable!(),
        }
    }

    fn write_u16(&mut self, addr: u32, data: u16) {
        log::info!("write interrupts 16, regs {:X} = {:08X}", addr, data);
        match addr {
            0 => {
                self.stat = InterruptRegister::from_bits_truncate(data);
                log::info!("write interrupts stat {:?}", self.stat);
            }
            2 => {}
            4 => {
                self.mask = InterruptRegister::from_bits_truncate(data);
                log::info!("write interrupts mask {:?}", self.mask);
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