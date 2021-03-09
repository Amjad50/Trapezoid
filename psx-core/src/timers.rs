use crate::memory::BusLine;

pub struct Timers {
    // 3 times each have 3 registers
    timers: [[u16; 3]; 3],
}

impl Default for Timers {
    fn default() -> Self {
        Self {
            timers: [[0; 3]; 3],
        }
    }
}

impl BusLine for Timers {
    fn read_u32(&mut self, addr: u32) -> u32 {
        let timer_index = (addr >> 4) & 0x3;
        let reg_index = (addr & 0xF) / 4;

        self.timers[timer_index as usize][reg_index as usize] as u32
    }

    fn write_u32(&mut self, addr: u32, data: u32) {
        let timer_index = (addr >> 4) & 0x3;
        let reg_index = (addr & 0xF) / 4;

        println!(
            "written timer register addr=0x{:X}, data=0x{:X}",
            addr, data
        );
        self.timers[timer_index as usize][reg_index as usize] = data as u16;
    }

    fn read_u16(&mut self, addr: u32) -> u16 {
        let timer_index = (addr >> 4) & 0x3;
        let is_inside_reg = ((addr & 0xF) / 2) % 2 == 0;
        let reg_index = (addr & 0xF) / 4;

        if is_inside_reg {
            self.timers[timer_index as usize][reg_index as usize]
        } else {
            0
        }
    }

    fn write_u16(&mut self, addr: u32, data: u16) {
        let timer_index = (addr >> 4) & 0x3;
        let is_inside_reg = ((addr & 0xF) / 2) % 2 == 0;
        let reg_index = (addr & 0xF) / 4;

        if is_inside_reg {
            println!(
                "written timer register addr=0x{:X}, data=0x{:X}",
                addr, data
            );
            self.timers[timer_index as usize][reg_index as usize] = data;
        } else {
            println!(
                "written timer to garbage addr=0x{:X}, data=0x{:X}",
                addr, data
            );
        }
    }

    fn read_u8(&mut self, _addr: u32) -> u8 {
        todo!()
    }

    fn write_u8(&mut self, _addr: u32, _data: u8) {
        todo!()
    }
}
