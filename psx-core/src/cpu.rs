mod instruction;
mod instructions_table;
mod register;

use instruction::{Instruction, Opcode};
use register::Registers;

pub trait CpuBusProvider {
    fn read_u32(&mut self, addr: u32) -> u32;
    fn write_u32(&mut self, addr: u32, data: u32);
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
        let instruction = bus.read_u32(self.regs.pc);
        let instruction = Instruction::from_u32(instruction);

        self.regs.pc += 4;

        println!("{:02X?}", instruction);
        self.execute_instruction(instruction, bus);
    }
}

impl Cpu {
    fn sign_extend(data: u16) -> u32 {
        data as i16 as i32 as u32
    }

    fn execute_instruction<P: CpuBusProvider>(&mut self, instruction: Instruction, bus: &mut P) {
        match instruction.opcode {
            //Opcode::Lb => {}
            //Opcode::Lbu => {}
            //Opcode::Lh => {}
            //Opcode::Lhu => {}
            //Opcode::Lw => {}
            //Opcode::Lwl => {}
            //Opcode::Lwr => {}
            //Opcode::Sb => {}
            //Opcode::Sh => {}
            Opcode::Sw => {
                let rs = self.regs.read_register(instruction.rs);
                let rt = self.regs.read_register(instruction.rt);
                // TODO: check if wrapping or not
                let computed_addr = rs + (instruction.imm16 as u32);
                bus.write_u32(computed_addr, rt);
            }
            //Opcode::Swl => {}
            //Opcode::Swr => {}
            //Opcode::Slt => {}
            //Opcode::Sltu => {}
            //Opcode::Slti => {}
            //Opcode::Sltiu => {}
            //Opcode::Addu => {}
            //Opcode::Add => {}
            //Opcode::Subu => {}
            //Opcode::Sub => {}
            Opcode::Addiu => {
                let rs = self.regs.read_register(instruction.rs);
                let result = rs.wrapping_add(Self::sign_extend(instruction.imm16));
                self.regs.write_register(instruction.rt, result);
            }
            //Opcode::Addi => {}
            //Opcode::And => {}
            //Opcode::Or => {}
            //Opcode::Xor => {}
            //Opcode::Nor => {}
            //Opcode::Andi => {}
            Opcode::Ori => {
                let rs = self.regs.read_register(instruction.rs);
                let result = rs | (instruction.imm16 as u32);
                self.regs.write_register(instruction.rt, result);
            }
            //Opcode::Xori => {}
            //Opcode::Sllv => {}
            //Opcode::Srlv => {}
            //Opcode::Srav => {}
            Opcode::Sll => {
                let rt = self.regs.read_register(instruction.rt);
                let result = rt << instruction.imm5;
                self.regs.write_register(instruction.rd, result);
            }
            //Opcode::Srl => {}
            //Opcode::Sra => {}
            Opcode::Lui => {
                let result = (instruction.imm16 as u32) << 16;
                self.regs.write_register(instruction.rt, result);
            }
            Opcode::J => {
                let base = self.regs.pc & 0xF0000000;
                let offset = instruction.imm26 * 4;

                self.regs.pc = base + offset;
            }
            //Opcode::Jal => {}
            //Opcode::Jr => {}
            //Opcode::Jalr => {}
            //Opcode::Beq => {}
            //Opcode::Bne => {}
            //Opcode::Bgtz => {}
            //Opcode::Blez => {}
            //Opcode::Bcondz => {}
            Opcode::Special => unreachable!(),
            Opcode::Invalid => unreachable!(),
            Opcode::NotImplemented => todo!("opcode not registered"),
            _ => todo!("unimplemented_instruction {:?}", instruction.opcode),
        }
    }
}
