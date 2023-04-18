use std::fmt;

#[derive(Copy, Clone, Debug, PartialEq)]
#[repr(u8)]
pub enum RegisterType {
    Zero = 0,
    At,
    V0,
    V1,
    A0,
    A1,
    A2,
    A3,
    T0,
    T1,
    T2,
    T3,
    T4,
    T5,
    T6,
    T7,
    S0,
    S1,
    S2,
    S3,
    S4,
    S5,
    S6,
    S7,
    T8,
    T9,
    K0,
    K1,
    Gp,
    Sp,
    Fp,
    Ra,
    Pc,
    Hi,
    Lo,
}

const REG_TYPES: [RegisterType; 35] = [
    RegisterType::Zero,
    RegisterType::At,
    RegisterType::V0,
    RegisterType::V1,
    RegisterType::A0,
    RegisterType::A1,
    RegisterType::A2,
    RegisterType::A3,
    RegisterType::T0,
    RegisterType::T1,
    RegisterType::T2,
    RegisterType::T3,
    RegisterType::T4,
    RegisterType::T5,
    RegisterType::T6,
    RegisterType::T7,
    RegisterType::S0,
    RegisterType::S1,
    RegisterType::S2,
    RegisterType::S3,
    RegisterType::S4,
    RegisterType::S5,
    RegisterType::S6,
    RegisterType::S7,
    RegisterType::T8,
    RegisterType::T9,
    RegisterType::K0,
    RegisterType::K1,
    RegisterType::Gp,
    RegisterType::Sp,
    RegisterType::Fp,
    RegisterType::Ra,
    RegisterType::Pc,
    RegisterType::Hi,
    RegisterType::Lo,
];

pub static CPU_REGISTERS: phf::Map<&'static str, RegisterType> = phf::phf_map! {
    "zero" => RegisterType::Zero,
    "at" => RegisterType::At,
    "v0" => RegisterType::V0,
    "v1" => RegisterType::V1,
    "a0" => RegisterType::A0,
    "a1" => RegisterType::A1,
    "a2" => RegisterType::A2,
    "a3" => RegisterType::A3,
    "t0" => RegisterType::T0,
    "t1" => RegisterType::T1,
    "t2" => RegisterType::T2,
    "t3" => RegisterType::T3,
    "t4" => RegisterType::T4,
    "t5" => RegisterType::T5,
    "t6" => RegisterType::T6,
    "t7" => RegisterType::T7,
    "s0" => RegisterType::S0,
    "s1" => RegisterType::S1,
    "s2" => RegisterType::S2,
    "s3" => RegisterType::S3,
    "s4" => RegisterType::S4,
    "s5" => RegisterType::S5,
    "s6" => RegisterType::S6,
    "s7" => RegisterType::S7,
    "t8" => RegisterType::T8,
    "t9" => RegisterType::T9,
    "k0" => RegisterType::K0,
    "k1" => RegisterType::K1,
    "gp" => RegisterType::Gp,
    "sp" => RegisterType::Sp,
    "fp" => RegisterType::Fp,
    "ra" => RegisterType::Ra,
    "pc" => RegisterType::Pc,
    "hi" => RegisterType::Hi,
    "lo" => RegisterType::Lo,
};

pub const ALL_REG_NAMES: [&str; 35] = [
    "zero", "at", "v0", "v1", "a0", "a1", "a2", "a3", "t0", "t1", "t2", "t3", "t4", "t5", "t6",
    "t7", "s0", "s1", "s2", "s3", "s4", "s5", "s6", "s7", "t8", "t9", "k0", "k1", "gp", "sp", "fp",
    "ra", "pc", "hi", "lo",
];

impl From<u8> for RegisterType {
    fn from(value: u8) -> Self {
        REG_TYPES[value as usize]
    }
}

impl fmt::Display for RegisterType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(ALL_REG_NAMES[*self as usize])
    }
}

pub struct Registers {
    pub(crate) general_regs: [u32; 32],

    pub(crate) pc: u32,
    pub(crate) hi: u32,
    pub(crate) lo: u32,
}

impl Registers {
    pub(crate) fn new() -> Self {
        Self {
            general_regs: [0; 32],

            pc: 0xBFC00000,
            hi: 0,
            lo: 0,
        }
    }

    #[inline]
    pub fn read(&self, ty: RegisterType) -> u32 {
        match ty {
            RegisterType::Zero => 0,
            RegisterType::Pc => self.pc,
            RegisterType::Hi => self.hi,
            RegisterType::Lo => self.lo,
            _ => self.read_general(ty as u8),
        }
    }

    #[inline]
    pub fn write(&mut self, ty: RegisterType, data: u32) {
        match ty {
            RegisterType::Zero => {}
            RegisterType::Pc => self.pc = data,
            RegisterType::Hi => self.hi = data,
            RegisterType::Lo => self.lo = data,
            _ => self.write_general(ty as u8, data),
        }
    }

    #[inline]
    pub(crate) fn read_general(&self, idx: u8) -> u32 {
        assert!(idx < 32);
        self.general_regs[idx as usize]
    }

    #[inline]
    pub(crate) fn write_general(&mut self, idx: u8, data: u32) {
        assert!(idx < 32);
        self.general_regs[idx as usize] = data;
        self.general_regs[0] = 0;
    }

    // special function, since the cpu is writing to ra directly on function calls
    // and returns
    #[inline]
    pub(crate) fn write_ra(&mut self, data: u32) {
        self.general_regs[RegisterType::Ra as usize] = data;
    }
}

impl std::fmt::Debug for Registers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "Registers:")?;

        // print PC and AT
        writeln!(
            f,
            "pc: {:08X}\t{:>4}: {:08X}",
            self.pc,
            RegisterType::from(1),
            self.general_regs[1]
        )?;
        // HI and LO
        writeln!(f, "hi: {:08X}\tlo: {:08X}", self.hi, self.lo)?;

        // print all other registers except the last two
        for i in 2..32 / 2 {
            writeln!(
                f,
                "{:>4}: {:08X}\t{:>4}: {:08X}",
                RegisterType::from(i),
                self.general_regs[i as usize],
                // -2 offset because we are not printing 0 (ZERO) and 1 (AT)
                RegisterType::from(i + 32 / 2 - 2),
                self.general_regs[(i + 32 / 2 - 2) as usize]
            )?;
        }
        // print the last two registers
        writeln!(
            f,
            "{:>4}: {:08X}\t{:>4}: {:08X}",
            RegisterType::from(30),
            self.general_regs[30],
            RegisterType::from(31),
            self.general_regs[31]
        )
    }
}
