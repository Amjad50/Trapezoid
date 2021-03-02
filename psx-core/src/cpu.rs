mod instruction;
mod instructions_table;
mod register;

use instruction::Instruction;
use register::Registers;

pub trait CpuBusProvider {
    fn read(&mut self, addr: u32) -> u8;
    fn write(&mut self, addr: u32, data: u8);
}

pub struct Cpu {
    regs: Registers,
}

impl Cpu {
    pub fn new() -> Self {
        Self {
            // reset value
            regs: Registers::new(),
        }
    }

    pub fn execute_next<P: CpuBusProvider>(&mut self, bus: &mut P) {
        let instruction = Self::read_u32(bus, self.regs.pc);
        let instruction = Instruction::from_u32(instruction);

        self.regs.pc += 4;

        println!("{:02X?}", instruction);
        self.execute_instruction(instruction, bus);
    }
}

impl Cpu {
    fn execute_instruction<P: CpuBusProvider>(&mut self, instruction: Instruction, bus: &mut P) {
        match instruction.opcode {
            //instruction::Opcode::Lb => {}
            //instruction::Opcode::Lbu => {}
            //instruction::Opcode::Lh => {}
            //instruction::Opcode::Lhu => {}
            //instruction::Opcode::Lw => {}
            //instruction::Opcode::Lwl => {}
            //instruction::Opcode::Lwr => {}
            //instruction::Opcode::Sb => {}
            //instruction::Opcode::Sh => {}
            //instruction::Opcode::Sw => {}
            //instruction::Opcode::Swl => {}
            //instruction::Opcode::Swr => {}
            //instruction::Opcode::Slt => {}
            //instruction::Opcode::Sltu => {}
            //instruction::Opcode::Slti => {}
            //instruction::Opcode::Sltiu => {}
            //instruction::Opcode::Addu => {}
            //instruction::Opcode::Add => {}
            //instruction::Opcode::Subu => {}
            //instruction::Opcode::Sub => {}
            //instruction::Opcode::Addiu => {}
            //instruction::Opcode::Addi => {}
            //instruction::Opcode::And => {}
            //instruction::Opcode::Or => {}
            //instruction::Opcode::Xor => {}
            //instruction::Opcode::Nor => {}
            //instruction::Opcode::Andi => {}
            instruction::Opcode::Ori => {
                let inp = self.regs.read_register(instruction.rs);
                let result = inp | (instruction.imm16 as u32);
                self.regs.write_register(instruction.rt, result);
            }
            //instruction::Opcode::Xori => {}
            //instruction::Opcode::Sllv => {}
            //instruction::Opcode::Srlv => {}
            //instruction::Opcode::Srav => {}
            //instruction::Opcode::Sll => {}
            //instruction::Opcode::Srl => {}
            //instruction::Opcode::Sra => {}
            instruction::Opcode::Lui => {
                let result = (instruction.imm16 as u32) << 16;
                self.regs.write_register(instruction.rt, result);
            }
            instruction::Opcode::Special => unreachable!(),
            instruction::Opcode::Invalid => unreachable!(),
            instruction::Opcode::NotImplemented => todo!("opcode not registered"),
            _ => todo!("unimplemented_instruction {:?}", instruction.opcode),
        }
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
