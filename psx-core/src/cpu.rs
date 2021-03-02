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

    fn execute_alu_reg<F>(&mut self, instruction: Instruction, handler: F)
    where
        F: FnOnce(u32, u32) -> u32,
    {
        let rs = self.regs.read_register(instruction.rs);
        let rt = self.regs.read_register(instruction.rt);

        let result = handler(rs, rt);
        self.regs.write_register(instruction.rd, result);
    }

    fn execute_alu_imm<F>(&mut self, instruction: Instruction, handler: F)
    where
        F: FnOnce(u32, &Instruction) -> u32,
    {
        let rs = self.regs.read_register(instruction.rs);
        let result = handler(rs, &instruction);
        self.regs.write_register(instruction.rt, result);
    }

    fn execute_instruction<P: CpuBusProvider>(&mut self, instruction: Instruction, bus: &mut P) {
        match instruction.opcode {
            //Opcode::Lb => {}
            //Opcode::Lbu => {}
            //Opcode::Lh => {}
            //Opcode::Lhu => {}
            Opcode::Lw => {
                let rs = self.regs.read_register(instruction.rs);
                // TODO: check if wrapping or not
                let computed_addr = rs + (instruction.imm16 as u32);
                let data = bus.read_u32(computed_addr);

                self.regs.write_register(instruction.rt, data);
            }
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
            Opcode::Slt => {
                let rs = self.regs.read_register(instruction.rs) as i32;
                let rt = self.regs.read_register(instruction.rt) as i32;

                self.regs.write_register(instruction.rd, (rs < rt) as u32);
            }
            Opcode::Sltu => {
                let rs = self.regs.read_register(instruction.rs);
                let rt = self.regs.read_register(instruction.rt);

                self.regs.write_register(instruction.rd, (rs < rt) as u32);
            }
            Opcode::Slti => {
                let rs = self.regs.read_register(instruction.rs) as i32;
                let imm = instruction.imm16 as i16 as i32;

                self.regs.write_register(instruction.rd, (rs < imm) as u32);
            }
            Opcode::Sltiu => {
                let rs = self.regs.read_register(instruction.rs);
                let imm = Self::sign_extend(instruction.imm16);

                self.regs.write_register(instruction.rd, (rs < imm) as u32);
            }
            Opcode::Addu => {
                self.execute_alu_reg(instruction, |rs, rt| rs.wrapping_add(rt));
            }
            //Opcode::Add => {}
            Opcode::Subu => {
                self.execute_alu_reg(instruction, |rs, rt| rs.wrapping_sub(rt));
            }
            //Opcode::Sub => {}
            Opcode::Addiu => {
                self.execute_alu_imm(instruction, |rs, instr| {
                    rs.wrapping_add(Self::sign_extend(instr.imm16))
                });
            }
            //Opcode::Addi => {}
            Opcode::And => {
                self.execute_alu_reg(instruction, |rs, rt| rs & rt);
            }
            Opcode::Or => {
                self.execute_alu_reg(instruction, |rs, rt| rs | rt);
            }
            Opcode::Xor => {
                self.execute_alu_reg(instruction, |rs, rt| rs ^ rt);
            }
            Opcode::Nor => {
                self.execute_alu_reg(instruction, |rs, rt| !(rs | rt));
            }
            Opcode::Andi => {
                self.execute_alu_imm(instruction, |rs, instr| rs & (instr.imm16 as u32));
            }
            Opcode::Ori => {
                self.execute_alu_imm(instruction, |rs, instr| rs | (instr.imm16 as u32));
            }
            Opcode::Xori => {
                self.execute_alu_imm(instruction, |rs, instr| rs ^ (instr.imm16 as u32));
            }
            Opcode::Sllv => {
                self.execute_alu_reg(instruction, |rs, rt| rt << (rs & 0x1F));
            }
            Opcode::Srlv => {
                self.execute_alu_reg(instruction, |rs, rt| rt >> (rs & 0x1F));
            }
            Opcode::Srav => {
                self.execute_alu_reg(instruction, |rs, rt| ((rs as i32) >> rt) as u32);
            }
            Opcode::Sll => {
                let rt = self.regs.read_register(instruction.rt);
                let result = rt << instruction.imm5;
                self.regs.write_register(instruction.rd, result);
            }
            Opcode::Srl => {
                let rt = self.regs.read_register(instruction.rt);
                let result = rt >> instruction.imm5;
                self.regs.write_register(instruction.rd, result);
            }
            Opcode::Sra => {
                let rt = self.regs.read_register(instruction.rt);
                let result = ((rt as i32) >> instruction.imm5) as u32;
                self.regs.write_register(instruction.rd, result);
            }
            Opcode::Lui => {
                let result = (instruction.imm16 as u32) << 16;
                self.regs.write_register(instruction.rt, result);
            }
            Opcode::Mult => {
                let rs = self.regs.read_register(instruction.rs) as i32 as i64;
                let rt = self.regs.read_register(instruction.rt) as i32 as i64;

                let result = (rs * rt) as u64;

                self.regs.hi = (result >> 32) as u32;
                self.regs.lo = result as u32;
            }
            Opcode::Multu => {
                let rs = self.regs.read_register(instruction.rs) as u64;
                let rt = self.regs.read_register(instruction.rt) as u64;

                let result = rs * rt;

                self.regs.hi = (result >> 32) as u32;
                self.regs.lo = result as u32;
            }
            Opcode::Div => {
                let rs = self.regs.read_register(instruction.rs) as i32 as i64;
                let rt = self.regs.read_register(instruction.rt) as i32 as i64;

                let div = (rs / rt) as u32;
                let remainder = (rs % rt) as u32;

                self.regs.hi = remainder;
                self.regs.lo = div;
            }
            Opcode::Divu => {
                let rs = self.regs.read_register(instruction.rs) as u64;
                let rt = self.regs.read_register(instruction.rt) as u64;

                let div = (rs / rt) as u32;
                let remainder = (rs % rt) as u32;

                self.regs.hi = remainder;
                self.regs.lo = div;
            }
            Opcode::Mfhi => {
                self.regs.write_register(instruction.rd, self.regs.hi);
            }
            Opcode::Mthi => {
                self.regs.hi = self.regs.read_register(instruction.rs);
            }
            Opcode::Mflo => {
                self.regs.write_register(instruction.rd, self.regs.lo);
            }
            Opcode::Mtlo => {
                self.regs.lo = self.regs.read_register(instruction.rs);
            }
            Opcode::J => {
                let base = self.regs.pc & 0xF0000000;
                let offset = instruction.imm26 * 4;

                self.regs.pc = base + offset;
            }
            //Opcode::Jal => {}
            Opcode::Jr => {
                self.regs.pc = self.regs.read_register(instruction.rs);
            }
            //Opcode::Jalr => {}
            //Opcode::Beq => {}
            //Opcode::Bne => {}
            //Opcode::Bgtz => {}
            //Opcode::Blez => {}
            //Opcode::Bcondz => {}
            //Opcode::Syscall => {}
            //Opcode::Break => {}
            Opcode::Special => unreachable!(),
            Opcode::Invalid => unreachable!(),
            Opcode::NotImplemented => todo!("opcode not registered"),
            _ => todo!("unimplemented_instruction {:?}", instruction.opcode),
        }
    }
}
