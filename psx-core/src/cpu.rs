mod instruction;
mod instructions_table;
mod register;

use crate::memory::BusLine;

use instruction::{Instruction, Opcode};
use register::Registers;

#[derive(Default)]
struct SystemControlCoprocessor {
    bpc: u32,
    bda: u32,
    jmp_dest: u32,
    dcic: u32,
    bad_vaddr: u32,
    bdam: u32,
    bpcm: u32,
    sr: u32,
    cause: u32,
    epc: u32,
    prid: u32,
}

impl SystemControlCoprocessor {
    pub fn read_ctrl(&self, num: u8) -> u32 {
        assert!(num <= 0x1F);
        // no contrl registers
        0
    }

    pub fn write_ctrl(&mut self, num: u8, data: u32) {
        assert!(num <= 0x1F);
        // no contrl registers
    }

    pub fn read_data(&self, num: u8) -> u32 {
        assert!(num <= 0x1F);

        match num {
            // FIXME: reading any of these causes reserved instruction exception
            0..=2 | 4 | 10 => 0, // N/A
            3 => self.bpc,
            5 => self.bda,
            6 => self.jmp_dest,
            7 => self.dcic,
            8 => self.bad_vaddr,
            9 => self.bdam,
            11 => self.bpcm,
            12 => self.sr,
            13 => self.cause,
            14 => self.epc,
            15 => self.prid,
            // When reading one of the garbage registers shortly after reading
            // a valid cop0 register, the garbage value is usually the same
            // as that of the valid register. When doing the read later on,
            // the return value is usually 00000020h, or when reading much
            // later it returns 00000040h, or even 00000100h.
            16..=31 => 0xFF,
            _ => unreachable!(),
        }
    }

    pub fn write_data(&mut self, num: u8, data: u32) {
        assert!(num <= 0x1F);

        match num {
            // FIXME: does writing produce reserved instruction exception?
            0..=2 | 4 | 10 => {}  // N/A
            6 | 8 | 13..=15 => {} // not writable
            3 => self.bpc = data,
            5 => self.bda = data,
            7 => self.dcic = data,
            9 => self.bdam = data,
            11 => self.bpcm = data,
            12 => self.sr = data,
            16..=31 => {} // garbage
            _ => unreachable!(),
        }
    }
}

pub struct Cpu {
    regs: Registers,
    cop0: SystemControlCoprocessor,
}

impl Cpu {
    pub fn new() -> Self {
        Self {
            // reset value
            regs: Registers::new(),
            cop0: SystemControlCoprocessor::default(),
        }
    }

    pub fn execute_next<P: BusLine>(&mut self, bus: &mut P) {
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

    fn execute_instruction<P: BusLine>(&mut self, instruction: Instruction, bus: &mut P) {
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
            //Opcode::Cop(_) => {}
            Opcode::Mfc(n) => {
                // TODO: implement the other COPs, only cop0 for now
                assert_eq!(n, 0);
                let rd_raw = instruction.rd_raw;
                let result = self.cop0.read_data(rd_raw);

                self.regs.write_register(instruction.rt, result);
            }
            Opcode::Cfc(n) => {
                // TODO: implement the other COPs, only cop0 for now
                assert_eq!(n, 0);
                let rd_raw = instruction.rd_raw;
                let result = self.cop0.read_ctrl(rd_raw);

                self.regs.write_register(instruction.rt, result);
            }
            Opcode::Mtc(n) => {
                // TODO: implement the other COPs, only cop0 for now
                assert_eq!(n, 0);
                let rd_raw = instruction.rd_raw;
                let rt = self.regs.read_register(instruction.rt);

                self.cop0.write_data(rd_raw, rt);
            }
            Opcode::Ctc(n) => {
                // TODO: implement the other COPs, only cop0 for now
                assert_eq!(n, 0);
                let rd_raw = instruction.rd_raw;
                let rt = self.regs.read_register(instruction.rt);

                self.cop0.write_ctrl(rd_raw, rt);
            }
            //Opcode::Bcf(_) => {}
            //Opcode::Bct(_) => {}
            //Opcode::Rfe => {}
            //Opcode::Lwc(_) => {}
            //Opcode::Swc(_) => {}
            Opcode::Special => unreachable!(),
            Opcode::Invalid => unreachable!(),
            _ => todo!("unimplemented_instruction {:?}", instruction.opcode),
        }
    }
}
