#[cfg(feature = "debugger")]
mod debugger;
mod instruction;
mod instructions_table;
mod register;

use crate::coprocessor::{Gte, SystemControlCoprocessor};
pub use crate::memory::BusLine;

pub use instruction::{Instruction, Opcode};
pub use register::{RegisterType, Registers};

#[cfg(feature = "debugger")]
#[cfg_attr(docsrs, doc(cfg(feature = "debugger")))]
pub use self::debugger::Debugger;

#[cfg(not(feature = "debugger"))]
struct Debugger;

#[cfg(not(feature = "debugger"))]
// dummy implementation when the debugger is disabled
impl Debugger {
    #[inline]
    pub fn new() -> Self {
        Self
    }

    #[inline]
    pub fn paused(&self) -> bool {
        false
    }

    #[inline]
    pub fn last_state(&self) -> CpuState {
        CpuState::Normal
    }

    #[inline]
    pub fn clear_state(&mut self) {}

    #[inline]
    pub fn handle_pending_processing<P: CpuBusProvider>(
        &mut self,
        _bus: &mut P,
        _regs: &Registers,
        _jumping: bool,
    ) {
    }

    pub fn trace_exception(&mut self, _addr: u32) {}

    #[inline]
    pub fn trace_instruction(
        &mut self,
        _regs: &Registers,
        _jumping: bool,
        _instruction: &Instruction,
    ) -> bool {
        false
    }

    pub fn trace_write(&mut self, _addr: u32, _bits: u8) {}

    pub fn trace_read(&mut self, _addr: u32, _bits: u8) {}

    pub fn call_stack(&self) -> &[u32] {
        &[]
    }
}

const SHELL_LOCATION: u32 = 0x80030000;

/// A specific Bus that is connected to the CPU directly, where it can send Interrupt messages
/// to it and also request DMA access.
///
/// The trait only tells if the device is requesting for DMA,
/// then the implementer should handle the DMA operation as needed.
///
/// It's done like that so that the CPU just need to know when it should be interrupted and allow
/// the other components to run the DMA, here, the CPU doesn't run the DMA itself.
pub trait CpuBusProvider: BusLine {
    fn pending_interrupts(&self) -> bool;
    fn should_run_dma(&self) -> bool;
}

#[derive(Debug, Clone, Copy)]
enum Exception {
    Interrupt = 0x00,
    AddressErrorLoad = 0x04,
    AddressErrorStore = 0x05,
    _BusErrorInstructionFetch = 0x06,
    _BusErrorDataLoadStore = 0x07,
    Syscall = 0x08,
    Breakpoint = 0x09,
    ReservedInstruction = 0x0A,
    _CoprocessorUnusable = 0x0B,
    ArithmeticOverflow = 0x0C,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum CpuState {
    /// Normal execution, no breakpoints
    Normal,

    #[cfg(feature = "debugger")]
    #[cfg_attr(docsrs, doc(cfg(feature = "debugger")))]
    /// Paused on an execution breakpoint, the pause happen BEFORE execution
    InstructionBreakpoint(u32),

    #[cfg(feature = "debugger")]
    #[cfg_attr(docsrs, doc(cfg(feature = "debugger")))]
    /// Paused on a write breakpoint, together with the value that was written
    /// the pause happen AFTER the operation
    WriteBreakpoint { addr: u32, bits: u8 },

    #[cfg(feature = "debugger")]
    #[cfg_attr(docsrs, doc(cfg(feature = "debugger")))]
    /// Paused on a read breakpoint
    /// the pause happen AFTER the operation
    ReadBreakpoint { addr: u32, bits: u8 },

    #[cfg(feature = "debugger")]
    #[cfg_attr(docsrs, doc(cfg(feature = "debugger")))]
    /// Paused after a single instruction was executed
    Step,

    #[cfg(feature = "debugger")]
    #[cfg_attr(docsrs, doc(cfg(feature = "debugger")))]
    /// Paused after a single instruction was executed, if the instruction is `Jal` or `Jalr`
    /// which is used for function calls, the pause will happen after the function returns,
    /// i.e. step over the function
    StepOver,

    #[cfg(feature = "debugger")]
    #[cfg_attr(docsrs, doc(cfg(feature = "debugger")))]
    /// Continue execution until the CPU exit the current function
    StepOut,
}

pub struct Cpu {
    regs: Registers,
    cop0: SystemControlCoprocessor,
    cop2: Gte,

    jump_dest_next: Option<u32>,

    elapsed_cycles: u32,

    shell_reached_before: bool,
    shell_reached_now: bool,
    current_instr_pc: u32,

    debugger: Debugger,
}

impl Cpu {
    pub fn new() -> Self {
        Self {
            // reset value
            regs: Registers::new(),
            cop0: SystemControlCoprocessor::default(),
            cop2: Gte::default(),
            jump_dest_next: None,

            elapsed_cycles: 0,
            shell_reached_before: false,
            shell_reached_now: false,
            current_instr_pc: 0,

            debugger: Debugger::new(),
        }
    }

    pub fn reset(&mut self) {
        self.regs = Registers::new();
        self.cop0 = SystemControlCoprocessor::default();
        self.cop2 = Gte::default();
        self.jump_dest_next = None;
        self.elapsed_cycles = 0;
        self.shell_reached_before = false;
        self.current_instr_pc = 0;
    }

    pub fn registers(&self) -> &Registers {
        &self.regs
    }

    pub fn registers_mut(&mut self) -> &mut Registers {
        &mut self.regs
    }

    #[cfg(feature = "debugger")]
    #[cfg_attr(docsrs, doc(cfg(feature = "debugger")))]
    pub fn debugger(&mut self) -> &mut Debugger {
        &mut self.debugger
    }

    // Attempt to run `instructions` number of instructions
    // it can return early depending on number of reasons, such as:
    // - `debugger` reached a breakpoint
    // - `bus` requested a DMA operation
    // - shell location is reached
    pub fn clock<P: CpuBusProvider>(&mut self, bus: &mut P, instructions: u32) -> (u32, CpuState) {
        self.shell_reached_now = false;
        let mut state = CpuState::Normal;

        // we only need to run this only once before any instruction, as this
        // is used to process any pending debugger commands
        self.debugger
            .handle_pending_processing(bus, &self.regs, self.jump_dest_next.is_some());

        let pending_interrupts = bus.pending_interrupts();
        self.check_and_execute_interrupt(pending_interrupts);

        for _ in 0..instructions {
            // notify the UI when the shell location is reached
            if !self.shell_reached_before && self.regs.pc == SHELL_LOCATION {
                self.shell_reached_before = true;
                self.shell_reached_now = true;
                log::info!("shell location reached");
                break;
            }

            if let Some(instruction) = self.bus_read_u32(bus, self.regs.pc) {
                let instruction = Instruction::from_u32(instruction, self.regs.pc);

                self.current_instr_pc = self.regs.pc;

                log::trace!(
                    "{:08X}: {}{}",
                    self.regs.pc,
                    if self.jump_dest_next.is_some() {
                        "_"
                    } else {
                        ""
                    },
                    instruction
                );

                // breakpoint hit
                if self.debugger.trace_instruction(
                    &self.regs,
                    self.jump_dest_next.is_some(),
                    &instruction,
                ) {
                    break;
                }

                self.regs.pc += 4;
                if let Some(jump_dest) = self.jump_dest_next.take() {
                    log::trace!("pc jump {:08X}", jump_dest);
                    self.regs.pc = jump_dest;
                }

                self.execute_instruction(&instruction, bus);
                self.regs.handle_delayed_load();

                if self.debugger.paused() {
                    break;
                }

                // exit so that we can run dma
                // Delaying the DMA can cause problems,
                // since if the DMA's SyncMode is `0`, it should run between
                // CPU cycles. This means that the following execution flow doesn't have
                // race conditions, becuase the DMA will run in between these two
                // CPU executions:
                // - CPU: Setup and start DMA channel 6 (SyncMode=0)
                // - CPU: Setup and start DMA channel 6 (SyncMode=0)
                //
                // Thus, if we delayed the execution of DMA even for
                // just a small amount, it would cause problems.
                //
                // TODO: maybe we should optimize this a bit more, so that we
                //       won't need to check on every CPU instruction
                if bus.should_run_dma() {
                    break;
                }
            }
        }

        if self.debugger.paused() {
            state = self.debugger.last_state();
            self.debugger.clear_state();
        }

        (std::mem::take(&mut self.elapsed_cycles), state)
    }

    pub fn is_shell_reached(&self) -> bool {
        self.shell_reached_now
    }
}

impl Cpu {
    fn execute_exception(&mut self, cause: Exception) {
        log::info!(
            "executing exception: {:?}, cause code: {:02X}",
            cause,
            cause as u8
        );

        let cause_code = cause as u8;

        let old_cause = self.cop0.read_cause();
        // remove the next jump
        let bd = self.jump_dest_next.take().is_some();
        let new_cause =
            (old_cause & 0x7FFFFF00) | ((bd as u32) << 31) | ((cause_code & 0x1F) as u32) << 2;
        self.cop0.write_cause(new_cause);

        // move the current exception enable to the next position
        let mut sr = self.cop0.read_sr();
        let first_two_bits = sr & 3;
        let second_two_bits = (sr >> 2) & 3;
        sr &= !0b111111;
        sr |= first_two_bits << 2;
        sr |= second_two_bits << 4;
        self.cop0.write_sr(sr);

        let bev = (sr >> 22) & 1 == 1;

        let jmp_vector = if bev { 0xBFC00180 } else { 0x80000080 };

        // TODO: check the written value to EPC
        let target_pc = match cause {
            Exception::Interrupt => {
                if bd {
                    // execute branch again
                    self.regs.pc - 4
                } else {
                    self.regs.pc
                }
            }
            _ => self.regs.pc - 4,
        };

        self.cop0.write_epc(target_pc);
        self.regs.pc = jmp_vector;
        self.regs.flush_delayed_load();
        self.debugger.trace_exception(target_pc);
    }

    fn check_and_execute_interrupt(&mut self, pending_interrupts: bool) {
        let sr = self.cop0.read_sr();
        // cause.10 is not a latch, so it should be updated continually
        let new_cause = (self.cop0.read_cause() & !0x400) | ((pending_interrupts as u32) << 10);
        self.cop0.write_cause(new_cause);

        // cause.10 is set and sr.10 and sr.0 are set, then execute the interrupt
        if pending_interrupts && (sr & 0x401 == 0x401) {
            self.execute_exception(Exception::Interrupt);
        }
    }

    fn sign_extend_16(data: u16) -> u32 {
        data as i16 as i32 as u32
    }

    fn sign_extend_8(data: u8) -> u32 {
        data as i8 as i32 as u32
    }

    #[inline]
    fn execute_alu_reg<F>(&mut self, instruction: &Instruction, handler: F)
    where
        F: FnOnce(u32, u32) -> (u32, bool),
    {
        let rs = self.regs.read_general(instruction.rs_raw);
        let rt = self.regs.read_general(instruction.rt_raw);

        let (result, overflow) = handler(rs, rt);
        if overflow {
            self.execute_exception(Exception::ArithmeticOverflow);
        } else {
            self.regs.write_general(instruction.rd_raw, result);
        }
    }

    #[inline]
    fn execute_alu_imm<F>(&mut self, instruction: &Instruction, handler: F)
    where
        F: FnOnce(u32, &Instruction) -> u32,
    {
        let rs = self.regs.read_general(instruction.rs_raw);
        let result = handler(rs, instruction);
        self.regs.write_general(instruction.rt_raw, result);
    }

    #[inline]
    fn execute_branch<F>(&mut self, instruction: &Instruction, have_rt: bool, handler: F)
    where
        F: FnOnce(i32, i32) -> bool,
    {
        let rs = self.regs.read_general(instruction.rs_raw) as i32;
        let rt = if have_rt {
            self.regs.read_general(instruction.rt_raw) as i32
        } else {
            0
        };
        let signed_imm16 = Self::sign_extend_16(instruction.imm16()).wrapping_mul(4);

        let should_jump = handler(rs, rt);
        if should_jump {
            self.jump_dest_next = Some(self.regs.pc.wrapping_add(signed_imm16));
        }
    }

    #[inline]
    fn execute_load<F>(&mut self, instruction: &Instruction, mut handler: F)
    where
        F: FnMut(&mut Self, u32) -> Option<u32>,
    {
        let rs = self.regs.read_general(instruction.rs_raw);
        let computed_addr = rs.wrapping_add(Self::sign_extend_16(instruction.imm16()));

        if let Some(data) = handler(self, computed_addr) {
            self.regs.write_delayed(instruction.rt_raw, data);
        }
    }

    #[inline]
    fn execute_store<F>(&mut self, instruction: &Instruction, mut handler: F)
    where
        F: FnMut(&mut Self, u32, u32),
    {
        let rs = self.regs.read_general(instruction.rs_raw);
        let rt = self.regs.read_general(instruction.rt_raw);
        let computed_addr = rs.wrapping_add(Self::sign_extend_16(instruction.imm16()));

        handler(self, computed_addr, rt);
    }

    fn execute_instruction<P: CpuBusProvider>(&mut self, instruction: &Instruction, bus: &mut P) {
        match instruction.opcode {
            Opcode::Nop => {
                // nothing
            }
            Opcode::Lb => {
                self.execute_load(instruction, |s, computed_addr| {
                    Some(Self::sign_extend_8(s.bus_read_u8(bus, computed_addr)))
                });
            }
            Opcode::Lbu => {
                self.execute_load(instruction, |s, computed_addr| {
                    Some(s.bus_read_u8(bus, computed_addr) as u32)
                });
            }
            Opcode::Lh => {
                self.execute_load(instruction, |s, computed_addr| {
                    Some(Self::sign_extend_16(s.bus_read_u16(bus, computed_addr)?))
                });
            }
            Opcode::Lhu => {
                self.execute_load(instruction, |s, computed_addr| {
                    Some(s.bus_read_u16(bus, computed_addr)? as u32)
                });
            }
            Opcode::Lw => {
                self.execute_load(instruction, |s, computed_addr| {
                    s.bus_read_u32(bus, computed_addr)
                });
            }
            Opcode::Lwl => {
                // TODO: test these unaligned addressing instructions
                let rs = self.regs.read_general(instruction.rs_raw);
                let computed_addr = rs.wrapping_add(Self::sign_extend_16(instruction.imm16()));

                // round to the nearest floor of four
                let start = computed_addr & !3;
                let end = computed_addr;
                let offset = computed_addr & 3;
                let mut result = 0;

                // read the data in little endian
                for part_addr in (start..=end).rev() {
                    result <<= 8;
                    result |= self.bus_read_u8(bus, part_addr) as u32;
                }
                // move it to the upper part
                let shift = (3 - offset) * 8;
                result <<= shift;

                let mask = !((0xFFFFFFFF >> shift) << shift);
                let original_rt = self.regs.read_general_latest(instruction.rt_raw);
                let result = (original_rt & mask) | result;

                self.regs.write_delayed(instruction.rt_raw, result);
            }
            Opcode::Lwr => {
                let rs = self.regs.read_general(instruction.rs_raw);
                let computed_addr = rs.wrapping_add(Self::sign_extend_16(instruction.imm16()));

                let start = computed_addr;
                let end = computed_addr | 3;
                let offset = computed_addr & 3;
                let mut result = 0;

                // read the data in little endian
                for part_addr in (start..=end).rev() {
                    result <<= 8;
                    result |= self.bus_read_u8(bus, part_addr) as u32;
                }
                // move it to the upper part
                let shift = offset * 8;

                let mask = !(0xFFFFFFFF >> shift);
                let original_rt = self.regs.read_general_latest(instruction.rt_raw);
                let result = (original_rt & mask) | result;

                self.regs.write_delayed(instruction.rt_raw, result);
            }
            Opcode::Sb => {
                self.execute_store(instruction, |s, computed_addr, data| {
                    s.bus_write_u8(bus, computed_addr, data as u8)
                });
            }
            Opcode::Sh => {
                self.execute_store(instruction, |s, computed_addr, data| {
                    s.bus_write_u16(bus, computed_addr, data as u16)
                });
            }
            Opcode::Sw => {
                self.execute_store(instruction, |s, computed_addr, data| {
                    s.bus_write_u32(bus, computed_addr, data)
                });
            }
            Opcode::Swl => {
                let rs = self.regs.read_general(instruction.rs_raw);
                let mut rt = self.regs.read_general(instruction.rt_raw);
                let computed_addr = rs.wrapping_add(Self::sign_extend_16(instruction.imm16()));

                // round to the nearest floor of four
                let start = computed_addr & !3;
                let end = computed_addr;
                let offset = computed_addr & 3;

                // move it from the upper part
                let shift = (3 - offset) * 8;
                rt >>= shift;

                // write the data in little endian
                for part_addr in start..=end {
                    self.bus_write_u8(bus, part_addr, rt as u8);
                    rt >>= 8;
                }
            }
            Opcode::Swr => {
                let rs = self.regs.read_general(instruction.rs_raw);
                let mut rt = self.regs.read_general(instruction.rt_raw);
                let computed_addr = rs.wrapping_add(Self::sign_extend_16(instruction.imm16()));

                let start = computed_addr;
                let end = computed_addr | 3;

                // write the data in little endian
                for part_addr in start..=end {
                    self.bus_write_u8(bus, part_addr, rt as u8);
                    rt >>= 8;
                }
            }
            Opcode::Slt => {
                let rs = self.regs.read_general(instruction.rs_raw) as i32;
                let rt = self.regs.read_general(instruction.rt_raw) as i32;

                self.regs
                    .write_general(instruction.rd_raw, (rs < rt) as u32);
            }
            Opcode::Sltu => {
                let rs = self.regs.read_general(instruction.rs_raw);
                let rt = self.regs.read_general(instruction.rt_raw);

                self.regs
                    .write_general(instruction.rd_raw, (rs < rt) as u32);
            }
            Opcode::Slti => {
                let rs = self.regs.read_general(instruction.rs_raw) as i32;
                let imm = instruction.imm16() as i16 as i32;

                self.regs
                    .write_general(instruction.rt_raw, (rs < imm) as u32);
            }
            Opcode::Sltiu => {
                let rs = self.regs.read_general(instruction.rs_raw);
                let imm = Self::sign_extend_16(instruction.imm16());

                self.regs
                    .write_general(instruction.rt_raw, (rs < imm) as u32);
            }
            Opcode::Addu => {
                self.execute_alu_reg(instruction, |rs, rt| (rs.wrapping_add(rt), false));
            }
            Opcode::Add => {
                self.execute_alu_reg(instruction, |rs, rt| {
                    let (value, overflow) = (rs as i32).overflowing_add(rt as i32);
                    (value as u32, overflow)
                });
            }
            Opcode::Subu => {
                self.execute_alu_reg(instruction, |rs, rt| (rs.wrapping_sub(rt), false));
            }
            Opcode::Sub => {
                self.execute_alu_reg(instruction, |rs, rt| {
                    let (value, overflow) = (rs as i32).overflowing_sub(rt as i32);
                    (value as u32, overflow)
                });
            }
            Opcode::Addiu => {
                self.execute_alu_imm(instruction, |rs, instr| {
                    rs.wrapping_add(Self::sign_extend_16(instr.imm16()))
                });
            }
            Opcode::Addi => {
                let rs = self.regs.read_general(instruction.rs_raw);
                let (result, overflow) =
                    (rs as i32).overflowing_add(Self::sign_extend_16(instruction.imm16()) as i32);

                if overflow {
                    self.execute_exception(Exception::ArithmeticOverflow);
                } else {
                    self.regs.write_general(instruction.rt_raw, result as u32);
                }
            }
            Opcode::And => {
                self.execute_alu_reg(instruction, |rs, rt| (rs & rt, false));
            }
            Opcode::Or => {
                self.execute_alu_reg(instruction, |rs, rt| (rs | rt, false));
            }
            Opcode::Xor => {
                self.execute_alu_reg(instruction, |rs, rt| (rs ^ rt, false));
            }
            Opcode::Nor => {
                self.execute_alu_reg(instruction, |rs, rt| (!(rs | rt), false));
            }
            Opcode::Andi => {
                self.execute_alu_imm(instruction, |rs, instr| rs & (instr.imm16() as u32));
            }
            Opcode::Ori => {
                self.execute_alu_imm(instruction, |rs, instr| rs | (instr.imm16() as u32));
            }
            Opcode::Xori => {
                self.execute_alu_imm(instruction, |rs, instr| rs ^ (instr.imm16() as u32));
            }
            Opcode::Sllv => {
                self.execute_alu_reg(instruction, |rs, rt| (rt << (rs & 0x1F), false));
            }
            Opcode::Srlv => {
                self.execute_alu_reg(instruction, |rs, rt| (rt >> (rs & 0x1F), false));
            }
            Opcode::Srav => {
                self.execute_alu_reg(instruction, |rs, rt| {
                    (((rt as i32) >> (rs & 0x1F)) as u32, false)
                });
            }
            Opcode::Sll => {
                let rt = self.regs.read_general(instruction.rt_raw);
                let result = rt << instruction.imm5();
                self.regs.write_general(instruction.rd_raw, result);
            }
            Opcode::Srl => {
                let rt = self.regs.read_general(instruction.rt_raw);
                let result = rt >> instruction.imm5();
                self.regs.write_general(instruction.rd_raw, result);
            }
            Opcode::Sra => {
                let rt = self.regs.read_general(instruction.rt_raw);
                let result = ((rt as i32) >> instruction.imm5()) as u32;
                self.regs.write_general(instruction.rd_raw, result);
            }
            Opcode::Lui => {
                let result = (instruction.imm16() as u32) << 16;
                self.regs.write_general(instruction.rt_raw, result);
            }
            Opcode::Mult => {
                self.elapsed_cycles += 5;
                let rs = self.regs.read_general(instruction.rs_raw) as i32 as i64;
                let rt = self.regs.read_general(instruction.rt_raw) as i32 as i64;

                let result = (rs * rt) as u64;

                self.regs.hi = (result >> 32) as u32;
                self.regs.lo = result as u32;
            }
            Opcode::Multu => {
                self.elapsed_cycles += 5;
                let rs = self.regs.read_general(instruction.rs_raw) as u64;
                let rt = self.regs.read_general(instruction.rt_raw) as u64;

                let result = rs * rt;

                self.regs.hi = (result >> 32) as u32;
                self.regs.lo = result as u32;
            }
            Opcode::Div => {
                self.elapsed_cycles += 10;
                let rs = self.regs.read_general(instruction.rs_raw) as i32 as i64;
                let rt = self.regs.read_general(instruction.rt_raw) as i32 as i64;

                // division by zero (overflow)
                if rt == 0 {
                    self.regs.hi = rs as u32;
                    // -1 or 1
                    self.regs.lo = if rs >= 0 { 0xFFFFFFFF } else { 1 };
                } else {
                    let div = (rs / rt) as u32;
                    let remainder = (rs % rt) as u32;

                    self.regs.hi = remainder;
                    self.regs.lo = div;
                }
            }
            Opcode::Divu => {
                self.elapsed_cycles += 10;
                let rs = self.regs.read_general(instruction.rs_raw) as u64;
                let rt = self.regs.read_general(instruction.rt_raw) as u64;

                // division by zero
                if rt == 0 {
                    self.regs.hi = rs as u32;
                    self.regs.lo = 0xFFFFFFFF;
                } else {
                    let div = (rs / rt) as u32;
                    let remainder = (rs % rt) as u32;

                    self.regs.hi = remainder;
                    self.regs.lo = div;
                }
            }
            Opcode::Mfhi => {
                self.regs.write_general(instruction.rd_raw, self.regs.hi);
            }
            Opcode::Mthi => {
                self.regs.hi = self.regs.read_general(instruction.rs_raw);
            }
            Opcode::Mflo => {
                self.regs.write_general(instruction.rd_raw, self.regs.lo);
            }
            Opcode::Mtlo => {
                self.regs.lo = self.regs.read_general(instruction.rs_raw);
            }
            Opcode::J => {
                let base = self.regs.pc & 0xF0000000;
                let offset = instruction.imm26() * 4;

                self.jump_dest_next = Some(base + offset);
            }
            Opcode::Jal => {
                let base = self.regs.pc & 0xF0000000;
                let offset = instruction.imm26() * 4;

                self.jump_dest_next = Some(base + offset);

                self.regs.write_ra(self.regs.pc + 4);
            }
            Opcode::Jr => {
                self.jump_dest_next = Some(self.regs.read_general(instruction.rs_raw));
            }
            Opcode::Jalr => {
                self.jump_dest_next = Some(self.regs.read_general(instruction.rs_raw));

                self.regs
                    .write_general(instruction.rd_raw, self.regs.pc + 4);
            }
            Opcode::Beq => {
                self.execute_branch(instruction, true, |rs, rt| rs == rt);
            }
            Opcode::Bne => {
                self.execute_branch(instruction, true, |rs, rt| rs != rt);
            }
            Opcode::Bgtz => {
                self.execute_branch(instruction, false, |rs, _| rs > 0);
            }
            Opcode::Blez => {
                self.execute_branch(instruction, false, |rs, _| rs <= 0);
            }
            Opcode::Bltz => {
                self.execute_branch(instruction, false, |rs, _| rs < 0);
            }
            Opcode::Bgez => {
                self.execute_branch(instruction, false, |rs, _| rs >= 0);
            }
            Opcode::Bltzal => {
                self.execute_branch(instruction, false, |rs, _| rs < 0);
                // modify ra either way
                self.regs.write_ra(self.regs.pc + 4);
            }
            Opcode::Bgezal => {
                self.execute_branch(instruction, false, |rs, _| rs >= 0);
                // modify ra either way
                self.regs.write_ra(self.regs.pc + 4);
            }
            Opcode::Bcondz => unreachable!("bcondz should be converted"),
            Opcode::Syscall => {
                self.execute_exception(Exception::Syscall);
            }
            Opcode::Break => {
                self.execute_exception(Exception::Breakpoint);
            }
            Opcode::Cop(n) => {
                // the only cop0 command RFE is handled as its own opcode
                // so we only handle cop2 commands
                assert!(n == 2);

                self.cop2.execute_command(instruction.imm25());
            }
            Opcode::Mfc(n) => {
                let result = match n {
                    0 => self.cop0.read_data(instruction.rd_raw),
                    2 => self.cop2.read_data(instruction.rd_raw),
                    _ => unreachable!(),
                };

                self.regs.write_general(instruction.rt_raw, result);
            }
            Opcode::Cfc(n) => {
                let result = match n {
                    0 => self.cop0.read_ctrl(instruction.rd_raw),
                    2 => self.cop2.read_ctrl(instruction.rd_raw),
                    _ => unreachable!(),
                };

                self.regs.write_general(instruction.rt_raw, result);
            }
            Opcode::Mtc(n) => {
                let rt = self.regs.read_general(instruction.rt_raw);

                match n {
                    0 => self.cop0.write_data(instruction.rd_raw, rt),
                    2 => self.cop2.write_data(instruction.rd_raw, rt),
                    _ => unreachable!(),
                }
            }
            Opcode::Ctc(n) => {
                let rt = self.regs.read_general(instruction.rt_raw);

                match n {
                    0 => self.cop0.write_ctrl(instruction.rd_raw, rt),
                    2 => self.cop2.write_ctrl(instruction.rd_raw, rt),
                    _ => unreachable!(),
                }
            }
            //Opcode::Bcf(_) => {}
            //Opcode::Bct(_) => {}
            Opcode::Rfe => {
                let mut sr = self.cop0.read_sr();
                // clear first two bits
                let second_two_bits = (sr >> 2) & 3;
                let third_two_bits = (sr >> 4) & 3;
                sr &= !0b1111;
                sr |= second_two_bits;
                sr |= third_two_bits << 2;

                self.cop0.write_sr(sr);
            }
            Opcode::Lwc(n) => {
                let rs = self.regs.read_general(instruction.rs_raw);
                let computed_addr = rs.wrapping_add(Self::sign_extend_16(instruction.imm16()));

                if let Some(data) = self.bus_read_u32(bus, computed_addr) {
                    match n {
                        0 => self.cop0.write_data(instruction.rt_raw, data),
                        2 => self.cop2.write_data(instruction.rt_raw, data),
                        _ => unreachable!(),
                    }
                }
            }
            Opcode::Swc(n) => {
                let result = match n {
                    0 => self.cop0.read_data(instruction.rt_raw),
                    2 => self.cop2.read_data(instruction.rt_raw),
                    _ => unreachable!(),
                };

                self.execute_store(instruction, |s, computed_addr, _| {
                    s.bus_write_u32(bus, computed_addr, result);
                });
            }
            Opcode::Invalid => {
                self.execute_exception(Exception::ReservedInstruction);
            }
            Opcode::SecondaryOpcode => unreachable!(),
            _ => todo!("unimplemented_instruction {:?}", instruction.opcode),
        }
    }
}

impl Cpu {
    fn print_call_stack(&self) {
        let call_stack = self.debugger.call_stack();

        if call_stack.is_empty() {
            log::error!("call stack is empty");
        } else {
            log::error!("call stack:");
            for (i, pc) in call_stack.iter().enumerate() {
                log::error!("  {:02}: {:08X}", i, pc);
            }
        }
    }

    fn bus_read_u32<P: BusLine>(&mut self, bus: &mut P, addr: u32) -> Option<u32> {
        self.elapsed_cycles += 2;

        if addr % 4 != 0 {
            log::error!(
                "AddressErrorLoad(u32): {:08X} at {:08X}",
                addr,
                self.current_instr_pc
            );
            self.execute_exception(Exception::AddressErrorLoad);
            self.cop0.write_bad_vaddr(addr);

            return None;
        }

        self.debugger.trace_read(addr, 32);
        match addr {
            0x00000000..=0x00001000 if self.cop0.is_cache_isolated() => Some(0),
            _ => {
                let r = bus.read_u32(addr);
                match r {
                    Ok(value) => Some(value),
                    Err(err) => {
                        log::error!(
                            "bus_read_u32: {:08X} at {:08X}: {}",
                            addr,
                            self.current_instr_pc,
                            err
                        );
                        self.print_call_stack();
                        None
                    }
                }
            }
        }
    }

    fn bus_write_u32<P: BusLine>(&mut self, bus: &mut P, addr: u32, data: u32) {
        self.elapsed_cycles += 1;

        if addr % 4 != 0 {
            log::error!(
                "AddressErrorStore(u32): {:08X} at {:08X}",
                addr,
                self.current_instr_pc
            );
            self.execute_exception(Exception::AddressErrorStore);
            self.cop0.write_bad_vaddr(addr);
        } else {
            self.debugger.trace_write(addr, 32);
            match addr {
                0x00000000..=0x00001000 if self.cop0.is_cache_isolated() => {}
                _ => {
                    let r = bus.write_u32(addr, data);
                    if let Err(err) = r {
                        log::error!(
                            "bus_write_u32: {:08X} at {:08X}: {}",
                            addr,
                            self.current_instr_pc,
                            err
                        );
                        self.print_call_stack();
                    }
                }
            }
        }
    }

    fn bus_read_u16<P: BusLine>(&mut self, bus: &mut P, addr: u32) -> Option<u16> {
        self.elapsed_cycles += 1;
        if addr % 2 != 0 {
            log::error!(
                "AddressErrorLoad(u16): {:08X} at {:08X}",
                addr,
                self.current_instr_pc
            );
            self.execute_exception(Exception::AddressErrorLoad);
            self.cop0.write_bad_vaddr(addr);

            return None;
        }

        self.debugger.trace_read(addr, 16);
        match addr {
            0x00000000..=0x00001000 if self.cop0.is_cache_isolated() => Some(0),
            _ => {
                let r = bus.read_u16(addr);
                match r {
                    Ok(value) => Some(value),
                    Err(err) => {
                        log::error!(
                            "bus_read_u16: {:08X} at {:08X}: {}",
                            addr,
                            self.current_instr_pc,
                            err
                        );
                        self.print_call_stack();
                        None
                    }
                }
            }
        }
    }

    fn bus_write_u16<P: BusLine>(&mut self, bus: &mut P, addr: u32, data: u16) {
        self.elapsed_cycles += 1;
        if addr % 2 != 0 {
            log::error!(
                "AddressErrorStore(u16): {:08X} at {:08X}",
                addr,
                self.current_instr_pc
            );
            self.execute_exception(Exception::AddressErrorStore);
            self.cop0.write_bad_vaddr(addr);
        } else {
            self.debugger.trace_write(addr, 16);
            match addr {
                0x00000000..=0x00001000 if self.cop0.is_cache_isolated() => {}
                _ => {
                    let r = bus.write_u16(addr, data);
                    if let Err(err) = r {
                        log::error!(
                            "bus_write_u16: {:08X} at {:08X}: {}",
                            addr,
                            self.current_instr_pc,
                            err
                        );
                        self.print_call_stack();
                    }
                }
            }
        }
    }

    fn bus_read_u8<P: BusLine>(&mut self, bus: &mut P, addr: u32) -> u8 {
        self.elapsed_cycles += 1;
        self.debugger.trace_read(addr, 8);
        match addr {
            0x00000000..=0x00001000 if self.cop0.is_cache_isolated() => 0,
            _ => {
                let r = bus.read_u8(addr);
                match r {
                    Ok(value) => value,
                    Err(err) => {
                        log::error!(
                            "bus_read_u8: {:08X} at {:08X}: {}",
                            addr,
                            self.current_instr_pc,
                            err
                        );
                        self.print_call_stack();
                        0
                    }
                }
            }
        }
    }

    fn bus_write_u8<P: BusLine>(&mut self, bus: &mut P, addr: u32, data: u8) {
        self.elapsed_cycles += 1;
        self.debugger.trace_write(addr, 8);
        match addr {
            0x00000000..=0x00001000 if self.cop0.is_cache_isolated() => {}
            _ => {
                let r = bus.write_u8(addr, data);
                if let Err(err) = r {
                    log::error!(
                        "bus_write_u8: {:08X} at {:08X}: {}",
                        addr,
                        self.current_instr_pc,
                        err
                    );
                    self.print_call_stack();
                }
            }
        }
    }
}
