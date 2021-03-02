#[derive(Copy, Clone, Debug)]
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

impl RegisterType {
    pub fn from_byte(ident: u8) -> Self {
        let ident = ident & 0x1F;

        match ident {
            0 => Self::Zero,
            1 => Self::At,
            2..=3 => Self::V(ident - 2),
            4..=7 => Self::A(ident - 4),
            8..=15 => Self::T(ident - 8),
            16..=23 => Self::S(ident - 16),
            24..=25 => Self::T(ident - 16), // t8 and t9
            26..=27 => Self::K(ident - 26),
            28 => Self::Gp,
            29 => Self::Sp,
            30 => Self::Fp,
            31 => Self::Ra,
            _ => unreachable!(),
        }
    }
}

pub struct Registers {
    pub at: u32,
    pub v: [u32; 2],
    pub a: [u32; 4],
    pub t: [u32; 10],
    pub s: [u32; 8],
    pub k: [u32; 2],
    pub gp: u32,
    pub sp: u32,
    pub fp: u32,
    pub ra: u32,

    pub pc: u32,
    pub hi: u32,
    pub lo: u32,
}

impl Registers {
    pub fn new() -> Self {
        Self {
            at: 0,
            v: [0; 2],
            a: [0; 4],
            t: [0; 10],
            s: [0; 8],
            k: [0; 2],
            gp: 0,
            sp: 0,
            fp: 0,
            ra: 0,

            pc: 0xBFC00000,
            hi: 0,
            lo: 0,
        }
    }

    pub fn read_register(&self, ty: RegisterType) -> u32 {
        match ty {
            RegisterType::Zero => 0,
            RegisterType::At => self.at,
            RegisterType::V(i) => self.v[i as usize],
            RegisterType::A(i) => self.a[i as usize],
            RegisterType::T(i) => self.t[i as usize],
            RegisterType::S(i) => self.s[i as usize],
            RegisterType::K(i) => self.k[i as usize],
            RegisterType::Gp => self.gp,
            RegisterType::Sp => self.sp,
            RegisterType::Fp => self.fp,
            RegisterType::Ra => self.ra,
        }
    }

    pub fn write_register(&mut self, ty: RegisterType, data: u32) {
        match ty {
            RegisterType::Zero => {}
            RegisterType::At => self.at = data,
            RegisterType::V(i) => self.v[i as usize] = data,
            RegisterType::A(i) => self.a[i as usize] = data,
            RegisterType::T(i) => self.t[i as usize] = data,
            RegisterType::S(i) => self.s[i as usize] = data,
            RegisterType::K(i) => self.k[i as usize] = data,
            RegisterType::Gp => self.gp = data,
            RegisterType::Sp => self.sp = data,
            RegisterType::Fp => self.fp = data,
            RegisterType::Ra => self.ra = data,
        };
    }
}
