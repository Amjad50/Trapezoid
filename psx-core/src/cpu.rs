mod instruction;
mod instructions_table;

use instruction::Instruction;

pub trait CpuBusProvider {
    fn read(&mut self, addr: u32) -> u8;
    fn write(&mut self, addr: u32, data: u8);
}

pub struct Cpu {
    reg_pc: u32,
}

impl Cpu {
    pub fn new() -> Self {
        Self {
            // reset value
            reg_pc: 0xBFC00000,
        }
    }

    pub fn execute_next<P: CpuBusProvider>(&mut self, bus: &mut P) {
        let instruction = Self::read_u32(bus, self.reg_pc);
        let instruction = Instruction::from_u32(instruction);

        self.reg_pc += 4;

        println!("{:02X?}", instruction);
    }
}

impl Cpu {
    fn read_u32<P: CpuBusProvider>(bus: &mut P, addr: u32) -> u32 {
        let mut result = 0;

        // little endian
        for new_addr in (addr..addr + 4).rev() {
            let byte = bus.read(new_addr);

            result <<= 8;
            result |= byte as u32;
        }

        result
    }
}
