use super::instructions_table::{PRIMARY_OPCODES, SECONDARY_OPCODES};

#[derive(Copy, Clone, Debug)]
pub enum Opcode {
    Special,
    Invalid,
    NotImplemented,

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

    // (u) means unsigned, (t) means overflow trap, (i) means immediate
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
}

#[derive(Debug)]
pub struct Instruction {
    opcode: Opcode,

    primary_identifier: u8,
    secondary_identifier: u8,

    imm5: u8,
    rd: u8,
    rt: u8,
    rs: u8,
    imm16: u16,
    imm26: u32,
}

impl Instruction {
    pub fn from_u32(instruction: u32) -> Self {
        let primary_identifier = (instruction >> 26) as u8;
        let secondary_identifier = instruction as u8 & 0x3F;
        let imm5 = (instruction >> 6) as u8 & 0x1F;
        let rd = (instruction >> 11) as u8 & 0x1F;
        let rt = (instruction >> 16) as u8 & 0x1F;
        let rs = (instruction >> 21) as u8 & 0x1F;
        // combination of the above
        let imm16 = instruction as u16;
        let imm26 = instruction & 0x3FFFFFF;

        let mut opcode = Self::get_opcode_from_primary(primary_identifier);

        // special
        if let Opcode::Special = opcode {
            opcode = Self::get_opcode_from_secondary(secondary_identifier);
        }

        Self {
            opcode,
            primary_identifier,
            secondary_identifier,
            imm5,
            rd,
            rt,
            rs,
            imm16,
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
}
