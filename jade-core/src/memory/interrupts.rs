use crate::memory::Result;

use super::BusLine;

bitflags::bitflags! {
    #[derive(Default, Debug, Clone, Copy)]
    struct InterruptFlags: u16 {
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

pub trait InterruptRequester {
    fn request_vblank(&mut self);
    fn request_cdrom(&mut self);
    fn request_dma(&mut self);
    fn request_timer0(&mut self);
    fn request_timer1(&mut self);
    fn request_timer2(&mut self);
    fn request_controller_mem_card(&mut self);
    fn request_spu(&mut self);
}

#[derive(Default)]
pub struct Interrupts {
    stat: InterruptFlags,
    mask: InterruptFlags,
}

impl Interrupts {
    pub fn pending_interrupts(&self) -> bool {
        !(self.stat & self.mask).is_empty()
    }
}

impl BusLine for Interrupts {
    fn read_u32(&mut self, addr: u32) -> Result<u32> {
        let r = match addr {
            0 => self.stat.bits() as u32,
            4 => self.mask.bits() as u32,
            _ => unreachable!(),
        };
        Ok(r)
    }

    fn write_u32(&mut self, addr: u32, data: u32) -> Result<()> {
        log::info!("write interrupts 32, regs {:X} = {:08X}", addr, data);
        match addr {
            0 => {
                let bits_to_keep = InterruptFlags::from_bits_retain(data as u16);
                self.stat &= bits_to_keep;
                log::info!("write interrupts stat {:?}", self.stat);
            }
            4 => {
                self.mask = InterruptFlags::from_bits_retain(data as u16);
                log::info!("write interrupts mask {:?}", self.mask);
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    fn read_u16(&mut self, addr: u32) -> Result<u16> {
        let r = match addr {
            0 => self.stat.bits(),
            2 => 0,
            4 => self.mask.bits(),
            6 => 0,
            _ => unreachable!(),
        };
        Ok(r)
    }

    fn write_u16(&mut self, addr: u32, data: u16) -> Result<()> {
        log::info!("write interrupts 16, regs {:X} = {:08X}", addr, data);
        match addr {
            0 => {
                let bits_to_keep = InterruptFlags::from_bits_retain(data);
                self.stat &= bits_to_keep;
                log::info!("write interrupts stat {:?}", self.stat);
            }
            2 => {}
            4 => {
                self.mask = InterruptFlags::from_bits_retain(data);
                log::info!("write interrupts mask {:?}", self.mask);
            }
            6 => {}
            _ => unreachable!(),
        }
        Ok(())
    }
}

impl InterruptRequester for Interrupts {
    fn request_vblank(&mut self) {
        log::info!("requesting VBLANK interrupt");
        self.stat.insert(InterruptFlags::VBLANK);
    }

    fn request_cdrom(&mut self) {
        log::info!("requesting CDROM interrupt");
        self.stat.insert(InterruptFlags::CDROM);
    }

    fn request_dma(&mut self) {
        log::info!("requesting DMA interrupt");
        self.stat.insert(InterruptFlags::DMA);
    }

    fn request_timer0(&mut self) {
        log::info!("requesting TIMER0 interrupt");
        self.stat.insert(InterruptFlags::TIMER0)
    }
    fn request_timer1(&mut self) {
        log::info!("requesting TIMER1 interrupt");
        self.stat.insert(InterruptFlags::TIMER1)
    }

    fn request_timer2(&mut self) {
        log::info!("requesting TIMER2 interrupt");
        self.stat.insert(InterruptFlags::TIMER2)
    }

    fn request_controller_mem_card(&mut self) {
        log::info!("requesting CONTROLLER_AND_MEMCARD interrupt");
        self.stat.insert(InterruptFlags::CONTROLLER_AND_MEMCARD);
    }

    fn request_spu(&mut self) {
        log::info!("requesting SPU interrupt");
        self.stat.insert(InterruptFlags::SPU);
    }
}
