use std::fmt;

use super::{
    instruction::{Instruction, Opcode},
    register::Register,
};

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

pub const GENERAL_REG_NAMES: [&str; 32] = [
    "zero", "at", "v0", "v1", "a0", "a1", "a2", "a3", "t0", "t1", "t2", "t3", "t4", "t5", "t6",
    "t7", "s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7", "t8", "t9", "k0", "k1", "gp", "sp", "fp",
    "ra",
];
#[allow(dead_code)]
pub const REG_PC_NAME: &str = "pc";
#[allow(dead_code)]
pub const REG_HI_NAME: &str = "hi";
#[allow(dead_code)]
pub const REG_LO_NAME: &str = "lo";

#[allow(dead_code)]
pub const ALL_REG_NAMES: [&str; 35] = [
    "zero", "at", "v0", "v1", "a0", "a1", "a2", "a3", "t0", "t1", "t2", "t3", "t4", "t5", "t6",
    "t7", "s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7", "t8", "t9", "k0", "k1", "gp", "sp", "fp",
    "ra", "pc", "hi", "lo",
];

impl fmt::Display for Register {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(GENERAL_REG_NAMES[self.idx() as usize])
    }
}

fn format_load_store(f: &mut fmt::Formatter, instr: &Instruction) -> fmt::Result {
    let opcode = instr.opcode;

    let src = instr.rs;
    let off = instr.imm16;
    let dst = instr.rt;

    write!(f, "{} {}, 0x{:04X}({})", opcode_str(opcode), dst, off, src)
}

fn format_alu(f: &mut fmt::Formatter, instr: &Instruction, imm: bool) -> fmt::Result {
    let opcode = instr.opcode;

    let (first, third) = if imm {
        (instr.rt, format!("0x{:04X}", instr.imm16))
    } else {
        (instr.rd, format!("{}", instr.rt))
    };

    write!(
        f,
        "{} {}, {}, {}",
        opcode_str(opcode),
        first,
        instr.rs,
        third
    )
}

fn format_shift(f: &mut fmt::Formatter, instr: &Instruction, imm: bool) -> fmt::Result {
    let opcode = instr.opcode;

    let third = if imm {
        format!("0x{:02X}", instr.imm5)
    } else {
        format!("{}", instr.rs)
    };

    write!(
        f,
        "{} {}, {}, {}",
        opcode_str(opcode),
        instr.rd,
        instr.rt,
        third
    )
}

fn format_mult_div(f: &mut fmt::Formatter, instr: &Instruction) -> fmt::Result {
    let opcode = instr.opcode;

    write!(f, "{} {}, {}", opcode_str(opcode), instr.rs, instr.rt)
}

fn format_branch(f: &mut fmt::Formatter, instr: &Instruction, rt: bool) -> fmt::Result {
    let opcode = instr.opcode;

    let dest = instr.imm16;

    if rt {
        write!(
            f,
            "{} {}, {}, 0x{:04X} => 0x{:08X}",
            opcode_str(opcode),
            instr.rs,
            instr.rt,
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
            instr.rs,
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

    write!(f, "{} {}, {}", opcode_str(opcode), instr.rt, instr.rd)
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
                self.rt,
                self.imm16
            ),
            Opcode::Mult | Opcode::Multu | Opcode::Div | Opcode::Divu => format_mult_div(f, self),
            Opcode::Mfhi => write!(f, "{} {}", opcode_str(self.opcode), self.rd),
            Opcode::Mthi => write!(f, "{} {}", opcode_str(self.opcode), self.rs),
            Opcode::Mflo => write!(f, "{} {}", opcode_str(self.opcode), self.rd),
            Opcode::Mtlo => write!(f, "{} {}", opcode_str(self.opcode), self.rs),
            Opcode::J => write!(
                f,
                "{} 0x{:07X} => 0x{:08X}",
                opcode_str(self.opcode),
                self.imm26,
                self.pc & 0xF0000000 | self.imm26 * 4
            ),
            Opcode::Jal => write!(
                f,
                "{} 0x{:07X} => 0x{:08X}",
                opcode_str(self.opcode),
                self.imm26,
                self.pc & 0xF0000000 | self.imm26 * 4
            ),
            Opcode::Jr => write!(f, "{} {}", opcode_str(self.opcode), self.rs),
            // some specs says "jalr rd,rs"
            Opcode::Jalr => write!(f, "{} {}, {}", opcode_str(self.opcode), self.rs, self.rd),
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
            Opcode::Cop(_) => write!(f, "{} 0x{:07X}", opcode_str(self.opcode), self.imm25),
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
