mod instruction;
mod instructions_table;
mod register;

use instruction::Instruction;
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
            instruction::Opcode::Sw => {
                let rs = self.regs.read_register(instruction.rs);
                let rt = self.regs.read_register(instruction.rt);
                // TODO: check if wrapping or not
                let computed_addr = rs + (instruction.imm16 as u32);
                bus.write_u32(computed_addr, rt);
            }
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
                let rs = self.regs.read_register(instruction.rs);
                let result = rs | (instruction.imm16 as u32);
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
