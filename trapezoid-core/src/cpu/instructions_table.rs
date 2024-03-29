use super::instruction::Opcode;
use super::instruction::Opcode::*;

pub(super) const PRIMARY_OPCODES: &[Opcode; 0x40] = &[
    // 0x00
    SecondaryOpcode,
    Bcondz,
    J,
    Jal,
    Beq,
    Bne,
    Blez,
    Bgtz,
    Addi,
    Addiu,
    Slti,
    Sltiu,
    Andi,
    Ori,
    Xori,
    Lui,
    // 0x10
    Cop(0),
    Cop(1),
    Cop(2),
    Cop(3),
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    // 0x20
    Lb,
    Lh,
    Lwl,
    Lw,
    Lbu,
    Lhu,
    Lwr,
    Invalid,
    Sb,
    Sh,
    Swl,
    Sw,
    Invalid,
    Invalid,
    Swr,
    Invalid,
    // 0x30
    Lwc(0),
    Lwc(1),
    Lwc(2),
    Lwc(3),
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Swc(0),
    Swc(1),
    Swc(2),
    Swc(3),
    Invalid,
    Invalid,
    Invalid,
    Invalid,
];

#[rustfmt::skip]
pub(super) const SECONDARY_OPCODES: &[Opcode; 0x40] = &[
    // 0x00
    Sll,
    Invalid,
    Srl,
    Sra,
    Sllv,
    Invalid,
    Srlv,
    Srav,
    Jr,
    Jalr,
    Invalid,
    Invalid,
    Syscall,
    Break,
    Invalid,
    Invalid,
    // 0x10
    Mfhi,
    Mthi,
    Mflo,
    Mtlo,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Mult,
    Multu,
    Div,
    Divu,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    // 0x20
    Add,
    Addu,
    Sub,
    Subu,
    And,
    Or,
    Xor,
    Nor,
    Invalid,
    Invalid,
    Slt,
    Sltu,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    // 0x30
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
    Invalid,
];
