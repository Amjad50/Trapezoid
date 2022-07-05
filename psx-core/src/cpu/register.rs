use std::fmt;

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum RegisterType {
    Zero,
    At,
    V(u8),
    A(u8),
    T(u8),
    S(u8),
    K(u8),
    Gp,
    Sp,
    Fp,
    Ra,
}

const REG_TYPES: [RegisterType; 32] = [
    RegisterType::Zero,
    RegisterType::At,
    RegisterType::V(0),
    RegisterType::V(1),
    RegisterType::A(0),
    RegisterType::A(1),
    RegisterType::A(2),
    RegisterType::A(3),
    RegisterType::T(0),
    RegisterType::T(1),
    RegisterType::T(2),
    RegisterType::T(3),
    RegisterType::T(4),
    RegisterType::T(5),
    RegisterType::T(6),
    RegisterType::T(7),
    RegisterType::S(0),
    RegisterType::S(1),
    RegisterType::S(2),
    RegisterType::S(3),
    RegisterType::S(4),
    RegisterType::S(5),
    RegisterType::S(6),
    RegisterType::S(7),
    RegisterType::T(8),
    RegisterType::T(9),
    RegisterType::K(0),
    RegisterType::K(1),
    RegisterType::Gp,
    RegisterType::Sp,
    RegisterType::Fp,
    RegisterType::Ra,
];

#[repr(transparent)]
#[derive(Copy, Clone)]
pub struct Register {
    // from 0 to 31 types
    idx: u8,
}

impl Register {
    #[inline]
    pub fn from_byte(idx: u8) -> Self {
        Register { idx: idx & 0x1F }
    }

    #[inline]
    pub fn idx(&self) -> u8 {
        self.idx
    }
}

impl From<Register> for RegisterType {
    #[inline]
    fn from(v: Register) -> Self {
        REG_TYPES[v.idx as usize]
    }
}

impl From<RegisterType> for Register {
    #[inline]
    fn from(v: RegisterType) -> Self {
        // This is used for debugging only, so its just simple and slow
        Register {
            idx: REG_TYPES.iter().position(|x| *x == v).unwrap() as u8,
        }
    }
}

impl fmt::Debug for Register {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        RegisterType::from(*self).fmt(f)
    }
}

pub struct Registers {
    pub general_regs: [u32; 32],

    pub pc: u32,
    pub hi: u32,
    pub lo: u32,
}

impl Registers {
    pub fn new() -> Self {
        Self {
            general_regs: [0; 32],

            pc: 0xBFC00000,
            hi: 0,
            lo: 0,
        }
    }

    #[inline]
    pub fn read_register(&self, ty: Register) -> u32 {
        self.general_regs[ty.idx as usize]
    }

    #[inline]
    pub fn write_register(&mut self, ty: Register, data: u32) {
        self.general_regs[ty.idx as usize] = data;
        self.general_regs[0] = 0;
    }

    // special function, since the cpu is writing to ra directly on function calls
    // and returns
    #[inline]
    pub fn write_ra(&mut self, data: u32) {
        // ra is at index 31
        self.general_regs[31] = data;
    }
}

impl Registers {
    pub fn debug_print(&self) {
        println!("Registers:");

        println!(
            "pc: {:08X}\t{:>4}: {:08X}",
            self.pc,
            Register::from_byte(1),
            self.general_regs[1]
        );
        println!("hi: {:08X}\tlo: {:08X}", self.hi, self.lo);

        for i in 2..32 / 2 {
            println!(
                "{:>4}: {:08X}\t{:>4}: {:08X}",
                Register::from_byte(i),
                self.general_regs[i as usize],
                Register::from_byte(i + 32 / 2),
                self.general_regs[(i + 32 / 2) as usize]
            );
        }
    }
}
