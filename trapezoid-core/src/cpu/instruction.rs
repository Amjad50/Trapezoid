use std::fmt;

use super::instructions_table::{PRIMARY_OPCODES, SECONDARY_OPCODES};
use super::RegisterType;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Opcode {
    #[doc(hidden)]
    /// This is a special opcode used when parsing the instruction,
    /// This is not a valid instruction and not avilable in any documentation,
    /// but is used as a middle step to parse the instruction if the instruction
    /// has a secondary opcode metadata
    SecondaryOpcode,
    Invalid,

    Nop,

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
    // depending on the value of `rt` it will execute:
    //  rt   | instr
    // ---------------
    //  0x00 | Bltz
    //  0x01 | Bgez
    //  0x10 | Bltzal
    //  0x11 | Bgezal
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

/// MIPS Instruction representation.
///
/// Do note that the fields may not always make sense as there are fields used
/// by some instructions and some not used.
///
/// For example, the instruction `Lw` may have the value for `rd` set to
/// `RegisterType::A1`, or `RegisterType::T1` and it shouldn't matter
/// since that instruction only uses `rs` and `rt`. So keep that in mind when
/// accessing the fields.
#[derive(Debug, Clone)]
pub struct Instruction {
    pub pc: u32,

    pub opcode: Opcode,

    pub instruction: u32,
    pub(crate) rd_raw: u8,
    pub(crate) rt_raw: u8,
    pub(crate) rs_raw: u8,
}

impl Instruction {
    pub fn from_u32(instruction: u32, pc: u32) -> Self {
        if instruction == 0 {
            return Self {
                pc,

                opcode: Opcode::Nop,
                instruction,
                rd_raw: 0,
                rt_raw: 0,
                rs_raw: 0,
            };
        }

        let primary_identifier = (instruction >> 26) as u8;
        let rd_raw = (instruction >> 11) as u8 & 0x1F;
        let rt_raw = (instruction >> 16) as u8 & 0x1F;
        let rs_raw = (instruction >> 21) as u8 & 0x1F;

        let opcode = Self::get_opcode_from_primary(primary_identifier);

        let opcode = match opcode {
            Opcode::SecondaryOpcode => {
                let secondary_identifier = instruction as u8 & 0x3F;
                Self::get_opcode_from_secondary(secondary_identifier)
            }
            Opcode::Cop(n) => {
                let secondary_identifier = instruction as u8 & 0x3F;
                Self::get_cop_opcode(n, secondary_identifier, rt_raw, rs_raw)
            }
            Opcode::Bcondz => Self::get_bcondz_opcode(rt_raw),
            _ => opcode,
        };

        Self {
            pc,

            opcode,
            instruction,
            rd_raw,
            rt_raw,
            rs_raw,
        }
    }

    pub fn is_branch(&self) -> bool {
        matches!(
            self.opcode,
            Opcode::J
                | Opcode::Jal
                | Opcode::Jalr
                | Opcode::Jr
                | Opcode::Beq
                | Opcode::Bne
                | Opcode::Bgtz
                | Opcode::Blez
                | Opcode::Bltz
                | Opcode::Bgez
                | Opcode::Bltzal
                | Opcode::Bgezal
        )
    }

    #[inline]
    pub fn rd(&self) -> RegisterType {
        RegisterType::from(self.rd_raw)
    }

    #[inline]
    pub fn rt(&self) -> RegisterType {
        RegisterType::from(self.rt_raw)
    }

    #[inline]
    pub fn rs(&self) -> RegisterType {
        RegisterType::from(self.rs_raw)
    }

    #[inline]
    pub const fn imm5(&self) -> u8 {
        (self.instruction >> 6) as u8 & 0x1F
    }

    #[inline]
    pub const fn imm16(&self) -> u16 {
        self.instruction as u16
    }

    #[inline]
    pub const fn imm25(&self) -> u32 {
        self.instruction & 0x1FFFFFF
    }

    #[inline]
    pub const fn imm26(&self) -> u32 {
        self.instruction & 0x3FFFFFF
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
            0x10 => Opcode::Bltzal,
            0x11 => Opcode::Bgezal,
            x if x & 1 == 0 => Opcode::Bltz,
            // x if x & 1 == 1
            _ => Opcode::Bgez,
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

const fn opcode_str(opcode: Opcode) -> &'static str {
    match opcode {
        Opcode::Nop => "nop",
        Opcode::Lb => "lb",
        Opcode::Lh => "lh",
        Opcode::Lw => "lw",
        Opcode::Lbu => "lbu",
        Opcode::Lhu => "lhu",
        Opcode::Sb => "sb",
        Opcode::Lwl => "lwl",
        Opcode::Lwr => "lwr",
        Opcode::Sh => "sh",
        Opcode::Sw => "sw",
        Opcode::Swl => "swl",
        Opcode::Swr => "swr",
        Opcode::Slt => "slt",
        Opcode::Sltu => "sltu",
        Opcode::Slti => "slti",
        Opcode::Sltiu => "sltiu",
        Opcode::Addu => "addu",
        Opcode::Add => "add",
        Opcode::Subu => "subu",
        Opcode::Sub => "sub",
        Opcode::Addiu => "addiu",
        Opcode::Addi => "addi",
        Opcode::And => "and",
        Opcode::Or => "or",
        Opcode::Xor => "xor",
        Opcode::Nor => "nor",
        Opcode::Andi => "andi",
        Opcode::Ori => "ori",
        Opcode::Xori => "xori",
        Opcode::Sllv => "sllv",
        Opcode::Srlv => "srlv",
        Opcode::Srav => "srav",
        Opcode::Sll => "sll",
        Opcode::Srl => "srl",
        Opcode::Sra => "sra",
        Opcode::Lui => "lui",
        Opcode::Mult => "mult",
        Opcode::Multu => "multu",
        Opcode::Div => "div",
        Opcode::Divu => "divu",
        Opcode::Mfhi => "mfhi",
        Opcode::Mthi => "mthi",
        Opcode::Mflo => "mflo",
        Opcode::Mtlo => "mtlo",
        Opcode::J => "j",
        Opcode::Jal => "jal",
        Opcode::Jr => "jr",
        Opcode::Jalr => "jalr",
        Opcode::Beq => "beq",
        Opcode::Bne => "bne",
        Opcode::Bgtz => "bgtz",
        Opcode::Blez => "blez",
        Opcode::Bcondz => "bcondz",
        Opcode::Bltz => "bltz",
        Opcode::Bgez => "bgez",
        Opcode::Bltzal => "bltzal",
        Opcode::Bgezal => "bgezal",
        Opcode::Syscall => "syscall",
        Opcode::Break => "break",
        Opcode::Cop(0) => "cop0",
        Opcode::Cop(1) => "cop1",
        Opcode::Cop(2) => "cop2",
        Opcode::Cop(3) => "cop3",
        Opcode::Mfc(0) => "mfc0",
        Opcode::Mfc(1) => "mfc1",
        Opcode::Mfc(2) => "mfc2",
        Opcode::Mfc(3) => "mfc3",
        Opcode::Cfc(0) => "cfc0",
        Opcode::Cfc(1) => "cfc1",
        Opcode::Cfc(2) => "cfc2",
        Opcode::Cfc(3) => "cfc3",
        Opcode::Mtc(0) => "mtc0",
        Opcode::Mtc(1) => "mtc1",
        Opcode::Mtc(2) => "mtc2",
        Opcode::Mtc(3) => "mtc3",
        Opcode::Ctc(0) => "ctc0",
        Opcode::Ctc(1) => "ctc1",
        Opcode::Ctc(2) => "ctc2",
        Opcode::Ctc(3) => "ctc3",
        Opcode::Bcf(0) => "bcf0",
        Opcode::Bcf(1) => "bcf1",
        Opcode::Bcf(2) => "bcf2",
        Opcode::Bcf(3) => "bcf3",
        Opcode::Bct(0) => "bct0",
        Opcode::Bct(1) => "bct1",
        Opcode::Bct(2) => "bct2",
        Opcode::Bct(3) => "bct3",
        Opcode::Rfe => "rfe",
        Opcode::Lwc(0) => "lwc0",
        Opcode::Lwc(1) => "lwc1",
        Opcode::Lwc(2) => "lwc2",
        Opcode::Lwc(3) => "lwc3",
        Opcode::Swc(0) => "swc0",
        Opcode::Swc(1) => "swc1",
        Opcode::Swc(2) => "swc2",
        Opcode::Swc(3) => "swc3",
        _ => unreachable!(),
    }
}

fn format_load_store(f: &mut fmt::Formatter, instr: &Instruction) -> fmt::Result {
    let opcode = instr.opcode;

    let src = instr.rs();
    let off = instr.imm16();
    let dst = instr.rt();

    write!(f, "{} {}, 0x{:04X}({})", opcode_str(opcode), dst, off, src)
}

fn format_alu(f: &mut fmt::Formatter, instr: &Instruction, imm: bool) -> fmt::Result {
    let opcode = instr.opcode;

    let (first, third) = if imm {
        (instr.rt(), format!("0x{:04X}", instr.imm16()))
    } else {
        (instr.rd(), format!("{}", instr.rt()))
    };

    write!(
        f,
        "{} {}, {}, {}",
        opcode_str(opcode),
        first,
        instr.rs(),
        third
    )
}

fn format_shift(f: &mut fmt::Formatter, instr: &Instruction, imm: bool) -> fmt::Result {
    let opcode = instr.opcode;

    let third = if imm {
        format!("0x{:02X}", instr.imm5())
    } else {
        format!("{}", instr.rs())
    };

    write!(
        f,
        "{} {}, {}, {}",
        opcode_str(opcode),
        instr.rd(),
        instr.rt(),
        third
    )
}

fn format_mult_div(f: &mut fmt::Formatter, instr: &Instruction) -> fmt::Result {
    let opcode = instr.opcode;

    write!(f, "{} {}, {}", opcode_str(opcode), instr.rs(), instr.rt())
}

fn format_branch(f: &mut fmt::Formatter, instr: &Instruction, rt: bool) -> fmt::Result {
    let opcode = instr.opcode;

    let dest = instr.imm16();

    if rt {
        write!(
            f,
            "{} {}, {}, 0x{:04X} => 0x{:08X}",
            opcode_str(opcode),
            instr.rs(),
            instr.rt(),
            dest,
            instr
                .pc
                .wrapping_add((dest as i16 as i32 as u32).wrapping_mul(4))
                .wrapping_add(4)
        )
    } else {
        write!(
            f,
            "{} {}, 0x{:04X} => 0x{:08X}",
            opcode_str(opcode),
            instr.rs(),
            dest,
            instr
                .pc
                .wrapping_add((dest as i16 as i32 as u32).wrapping_mul(4))
                .wrapping_add(4)
        )
    }
}

fn format_cop_ops(f: &mut fmt::Formatter, instr: &Instruction) -> fmt::Result {
    let opcode = instr.opcode;

    write!(f, "{} {}, {}", opcode_str(opcode), instr.rt(), instr.rd())
}

impl fmt::Display for Instruction {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.opcode {
            Opcode::Lb
            | Opcode::Lbu
            | Opcode::Lh
            | Opcode::Lhu
            | Opcode::Lw
            | Opcode::Lwl
            | Opcode::Lwr
            | Opcode::Sb
            | Opcode::Sh
            | Opcode::Sw
            | Opcode::Swl
            | Opcode::Swr => format_load_store(f, self),
            Opcode::Slt | Opcode::Sltu => format_alu(f, self, false),
            Opcode::Slti | Opcode::Sltiu => format_alu(f, self, true),
            Opcode::Addu | Opcode::Add | Opcode::Subu | Opcode::Sub => format_alu(f, self, false),
            Opcode::Addiu | Opcode::Addi => format_alu(f, self, true),
            Opcode::And | Opcode::Or | Opcode::Xor | Opcode::Nor => format_alu(f, self, false),
            Opcode::Andi | Opcode::Ori | Opcode::Xori => format_alu(f, self, true),
            Opcode::Sllv | Opcode::Srlv | Opcode::Srav => format_shift(f, self, false),
            Opcode::Sll | Opcode::Srl | Opcode::Sra => format_shift(f, self, true),
            Opcode::Lui => write!(
                f,
                "{} {}, 0x{:04X}",
                opcode_str(self.opcode),
                self.rt(),
                self.imm16()
            ),
            Opcode::Mult | Opcode::Multu | Opcode::Div | Opcode::Divu => format_mult_div(f, self),
            Opcode::Mfhi => write!(f, "{} {}", opcode_str(self.opcode), self.rd()),
            Opcode::Mthi => write!(f, "{} {}", opcode_str(self.opcode), self.rs()),
            Opcode::Mflo => write!(f, "{} {}", opcode_str(self.opcode), self.rd()),
            Opcode::Mtlo => write!(f, "{} {}", opcode_str(self.opcode), self.rs()),
            Opcode::J => write!(
                f,
                "{} 0x{:07X} => 0x{:08X}",
                opcode_str(self.opcode),
                self.imm26(),
                (self.pc & 0xF0000000) | (self.imm26() * 4)
            ),
            Opcode::Jal => write!(
                f,
                "{} 0x{:07X} => 0x{:08X}",
                opcode_str(self.opcode),
                self.imm26(),
                (self.pc & 0xF0000000) | (self.imm26() * 4)
            ),
            Opcode::Jr => write!(f, "{} {}", opcode_str(self.opcode), self.rs()),
            // some specs says "jalr rd,rs"
            Opcode::Jalr => write!(
                f,
                "{} {}, {}",
                opcode_str(self.opcode),
                self.rs(),
                self.rd()
            ),
            Opcode::Beq | Opcode::Bne => format_branch(f, self, true),
            Opcode::Bgtz
            | Opcode::Blez
            | Opcode::Bltz
            | Opcode::Bgez
            | Opcode::Bltzal
            | Opcode::Bgezal => format_branch(f, self, false),
            Opcode::Nop | Opcode::Syscall | Opcode::Break | Opcode::Rfe => {
                f.write_str(opcode_str(self.opcode))
            }
            Opcode::Cop(_) => write!(f, "{} 0x{:07X}", opcode_str(self.opcode), self.imm25()),
            Opcode::Mfc(_) | Opcode::Cfc(_) | Opcode::Mtc(_) | Opcode::Ctc(_) => {
                format_cop_ops(f, self)
            }
            Opcode::Bcf(_) => todo!(),
            Opcode::Bct(_) => todo!(),
            Opcode::Lwc(_) | Opcode::Swc(_) => format_load_store(f, self),
            Opcode::Invalid => write!(f, "Invalid instruction"),
            _ => unreachable!(),
        }
    }
}
