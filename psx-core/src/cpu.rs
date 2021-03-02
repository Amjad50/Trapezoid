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

        self.reg_pc += 4;

        let primary = (instruction >> 26) as u8;
        let secondary = instruction as u8 & 0x3F;
        let imm5 = (instruction >> 6) as u8 & 0x1F;
        let rd = (instruction >> 11) as u8 & 0x1F;
        let rt = (instruction >> 16) as u8 & 0x1F;
        let rs = (instruction >> 21) as u8 & 0x1F;
        // combination of the above
        let imm16 = instruction as u16;
        let imm26 = instruction & 0x3FFFFFF;

        println!("instrction 0x{0:08X}, 0b{0:032b}", instruction);
        println!(
            "{0:06b}({0:02X}) {1:05b}({1:02X}) {2:05b}({2:02X}) {3:05b}({3:02X}) {4:05b}({4:02X}) {5:06b}({5:02X})",
            primary, rs, rt, rd, imm5, secondary
        );
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
