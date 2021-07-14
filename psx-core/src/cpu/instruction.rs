use super::instructions_table::{PRIMARY_OPCODES, SECONDARY_OPCODES};
use super::register::RegisterType;

#[derive(Copy, Clone, Debug)]
pub enum Opcode {
    Special,
    Invalid,

    // (u) means unsigned, (t) means overflow trap, (i) means immediate
    Lb,
    Lbu,
    Lh,
    Lhu,
    Lw,

    Lwl,
    Lwr,

    Sb,
    Sh,
    Sw,

    Swl,
    Swr,

    Slt,
    Sltu,
    Slti,
    Sltiu,

    Addu,
    Add,
    Subu,
    Sub,
    Addiu,
    Addi,

    And,
    Or,
    Xor,
    Nor,
    Andi,
    Ori,
    Xori,

    Sllv,
    Srlv,
    Srav,
    Sll,
    Srl,
    Sra,
    Lui,

    Mult,
    Multu,
    Div,
    Divu,

    Mfhi,
    Mthi,
    Mflo,
    Mtlo,

    J,
    Jal,

    Jr,
    Jalr,

    Beq,
    Bne,
    Bgtz,
    Blez,
    /// depending on the value of `rt` it will execute:
    ///  rt   | instr
    /// ---------------
    ///  0x00 | Bltz
    ///  0x01 | Bgez
    ///  0x10 | Bltzal
    ///  0x11 | Bgezal
    Bcondz,
    Bltz,
    Bgez,
    Bltzal,
    Bgezal,

    Syscall,
    Break,

    Cop(u8),
    Mfc(u8),
    Cfc(u8),
    Mtc(u8),
    Ctc(u8),

    Bcf(u8),
    Bct(u8),

    // only for COP0
    Rfe,

    Lwc(u8),
    Swc(u8),
}

#[derive(Clone, Copy, Debug)]
pub struct Instruction {
    pub opcode: Opcode,

    pub imm5: u8,
    pub rd_raw: u8,
    pub rd: RegisterType,
    pub rt_raw: u8,
    pub rt: RegisterType,
    pub rs_raw: u8,
    pub rs: RegisterType,
    pub imm16: u16,
    pub imm25: u32,
    pub imm26: u32,
}

impl Instruction {
    pub fn from_u32(instruction: u32) -> Self {
        let primary_identifier = (instruction >> 26) as u8;
        let secondary_identifier = instruction as u8 & 0x3F;
        let imm5 = (instruction >> 6) as u8 & 0x1F;
        let rd_raw = (instruction >> 11) as u8 & 0x1F;
        let rd = RegisterType::from_byte(rd_raw);
        let rt_raw = (instruction >> 16) as u8 & 0x1F;
        let rt = RegisterType::from_byte(rt_raw);
        let rs_raw = (instruction >> 21) as u8 & 0x1F;
        let rs = RegisterType::from_byte(rs_raw);
        // combination of the above
        let imm16 = instruction as u16;
        let imm26 = instruction & 0x3FFFFFF;
        let imm25 = instruction & 0x1FFFFFF;

        let mut opcode = Self::get_opcode_from_primary(primary_identifier);

        // special
        if let Opcode::Special = opcode {
            opcode = Self::get_opcode_from_secondary(secondary_identifier);
        }

        if let Opcode::Cop(n) = opcode {
            opcode = Self::get_cop_opcode(n, secondary_identifier, rt_raw, rs_raw);
        }

        if let Opcode::Bcondz = opcode {
            opcode = Self::get_bcondz_opcode(rt_raw);
        }

        Self {
            opcode,
            imm5,
            rd_raw,
            rd,
            rt_raw,
            rt,
            rs_raw,
            rs,
            imm16,
            imm25,
            imm26,
        }
    }
}

impl Instruction {
    fn get_opcode_from_primary(primary: u8) -> Opcode {
        PRIMARY_OPCODES[primary as usize & 0x3F]
    }

    fn get_opcode_from_secondary(secondary: u8) -> Opcode {
        SECONDARY_OPCODES[secondary as usize & 0x3F]
    }

    fn get_bcondz_opcode(rt_raw: u8) -> Opcode {
        match rt_raw {
            0x00 => Opcode::Bltz,
            0x01 => Opcode::Bgez,
            0x10 => Opcode::Bltzal,
            0x11 => Opcode::Bgezal,
            _ => Opcode::Invalid,
        }
    }

    fn get_cop_opcode(cop_n: u8, secondary: u8, part_20_16: u8, part_25_21: u8) -> Opcode {
        match part_25_21 {
            0 if secondary == 0 => Opcode::Mfc(cop_n),
            2 if secondary == 0 => Opcode::Cfc(cop_n),
            4 if secondary == 0 => Opcode::Mtc(cop_n),
            6 if secondary == 0 => Opcode::Ctc(cop_n),
            8 => match part_20_16 {
                0 => Opcode::Bcf(cop_n),
                1 => Opcode::Bct(cop_n),
                _ => Opcode::Invalid,
            },
            _ if part_25_21 & 0x10 != 0 => {
                if cop_n == 0 {
                    if secondary == 0x10 && part_25_21 == 0x10 {
                        // TODO: should we use a separate opcode, or just forward
                        //  Cop(n)cmd imm25 into Cop0 which should produce `RFE`?
                        Opcode::Rfe
                    } else {
                        Opcode::Invalid
                    }
                } else {
                    Opcode::Cop(cop_n)
                }
            }
            _ => Opcode::Invalid,
        }
    }
}
