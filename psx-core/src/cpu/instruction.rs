use super::instructions_table::{PRIMARY_OPCODES, SECONDARY_OPCODES};
use super::register::RegisterType;

#[derive(Copy, Clone, Debug)]
pub enum Opcode {
    Special,
    Invalid,
    NotImplemented,

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
}

#[derive(Clone, Copy, Debug)]
pub struct Instruction {
    pub opcode: Opcode,

    pub imm5: u8,
    pub rd: RegisterType,
    pub rt: RegisterType,
    pub rs: RegisterType,
    pub imm16: u16,
    pub imm26: u32,
}

impl Instruction {
    pub fn from_u32(instruction: u32) -> Self {
        let primary_identifier = (instruction >> 26) as u8;
        let secondary_identifier = instruction as u8 & 0x3F;
        let imm5 = (instruction >> 6) as u8 & 0x1F;
        let rd = (instruction >> 11) as u8 & 0x1F;
        let rd = RegisterType::from_byte(rd);
        let rt = (instruction >> 16) as u8 & 0x1F;
        let rt = RegisterType::from_byte(rt);
        let rs = (instruction >> 21) as u8 & 0x1F;
        let rs = RegisterType::from_byte(rs);
        // combination of the above
        let imm16 = instruction as u16;
        let imm26 = instruction & 0x3FFFFFF;

        let mut opcode = Self::get_opcode_from_primary(primary_identifier);

        // special
        if let Opcode::Special = opcode {
            opcode = Self::get_opcode_from_secondary(secondary_identifier);
        }

        if let Opcode::NotImplemented = opcode {
            println!(
                "LOG: not implemented opcode: primary = {:02X}, secondary = {:02X}",
                primary_identifier, secondary_identifier
            );
        }

        Self {
            opcode,
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
