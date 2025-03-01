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

    /// load delay slots
    /// When executing a `load` instruction, the data will be here during the execution
    load_delay_slot_running: Option<(u8, u32)>,
    /// When the instruction is done, the slot is moved here
    /// which is committed to the next cycle
    load_delay_slot_committing: Option<(u8, u32)>,
}

impl Registers {
    pub(crate) fn new() -> Self {
        Self {
            general_regs: [0; 32],
            load_delay_slot_running: None,
            load_delay_slot_committing: None,

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
        if let Some((i, _)) = self.load_delay_slot_committing {
            if idx == i {
                log::warn!(
                    "Reg `{}` is still in the load delay slot, reading old value, could be a bug",
                    REG_TYPES[idx as usize]
                );
            }
        }
        self.general_regs[idx as usize]
    }

    /// Used by Lwl and Lwr to accumulate the delay but work correctly
    /// for same register access
    #[inline]
    pub(crate) fn read_general_latest(&self, idx: u8) -> u32 {
        assert!(idx < 32);
        if let Some((i, d)) = self.load_delay_slot_committing {
            if idx == i {
                return d;
            }
        }
        self.general_regs[idx as usize]
    }

    #[inline]
    pub(crate) fn write_general(&mut self, idx: u8, data: u32) {
        assert!(idx < 32);
        self.general_regs[idx as usize] = data;
        self.general_regs[0] = 0;

        // cancel the load, otherwise it will overwrite what we are writing now
        // and we don't want that
        if let Some((i, _)) = self.load_delay_slot_committing {
            if i == idx {
                self.load_delay_slot_committing = None;
            }
        }
    }

    #[inline]
    pub(crate) fn write_delayed(&mut self, idx: u8, data: u32) {
        assert!(idx < 32);
        assert!(self.load_delay_slot_running.is_none());
        // if we are about to commit the same register, ignore that
        if let Some((i, _)) = self.load_delay_slot_committing {
            if i == idx {
                self.load_delay_slot_committing = None;
            }
        }
        self.load_delay_slot_running = Some((idx, data));
    }

    #[inline]
    pub(crate) fn handle_delayed_load(&mut self) {
        if let Some((idx, data)) = self.load_delay_slot_committing.take() {
            self.write_general(idx, data);
        }
        self.load_delay_slot_committing = self.load_delay_slot_running.take();
    }

    #[inline]
    pub(crate) fn flush_delayed_load(&mut self) {
        self.handle_delayed_load();
        self.handle_delayed_load();
    }

    // special function, since the cpu is writing to ra directly on function calls
    // and returns
    #[inline]
    pub(crate) fn write_ra(&mut self, data: u32) {
        // go through the normal write function to handle the load delay slot
        self.write_general(RegisterType::Ra as u8, data);
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
