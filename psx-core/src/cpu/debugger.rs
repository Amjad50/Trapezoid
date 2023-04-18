use std::collections::HashSet;

use super::{
    instruction::{Instruction, Opcode},
    register::Registers,
    CpuBusProvider, CpuState,
};

pub struct Debugger {
    instruction_trace: bool,
    paused: bool,
    last_state: CpuState,

    call_stack: Vec<u32>,

    step_over_breakpoints: HashSet<u32>,
    instruction_breakpoints: HashSet<u32>,
    write_breakpoints: HashSet<u32>,
    read_breakpoints: HashSet<u32>,
    // currently on top of breakpoint, so ignore it and continue when unpaused
    // so that we don't get stuck in one instruction.
    in_breakpoint: bool,
    // allow to execute one instruction only
    step: bool,
    step_over: bool,

    last_instruction: Instruction,
}

impl Debugger {
    pub(crate) fn new() -> Self {
        Self {
            instruction_trace: false,
            paused: false,
            last_state: CpuState::Normal,

            call_stack: Vec::new(),

            step_over_breakpoints: HashSet::new(),
            instruction_breakpoints: HashSet::new(),
            write_breakpoints: HashSet::new(),
            read_breakpoints: HashSet::new(),
            in_breakpoint: false,
            step: false,
            step_over: false,

            last_instruction: Instruction::from_u32(0, 0),
        }
    }

    pub(crate) fn set_pause(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub(crate) fn paused(&self) -> bool {
        self.paused
    }

    pub(crate) fn last_state(&self) -> CpuState {
        self.last_state
    }

    pub(crate) fn clear_state(&mut self) {
        self.last_state = CpuState::Normal;
        self.paused = false;
    }

    /// Perform some processing that is required by the `Debugger` but require access
    /// to the `bus`, since we don't have that normally unless we are inside
    /// the `clock` function in the cpu.
    ///
    /// so outside code that wants to use the `Debugger` that require `bus` access, we will stack
    /// those operations and perform them here.
    ///
    /// This is called first by the `clock` function in the `Cpu`.
    ///
    pub(crate) fn handle_pending_processing<P: CpuBusProvider>(
        &mut self,
        bus: &mut P,
        regs: &Registers,
        jumping: bool,
    ) {
        // need to step over
        if self.step_over {
            self.step_over = false;

            let offset;
            // check that the instruction we just executed is `Jal/r` and we are in the middle
            // of jump
            //
            // If so, we need to check the previous instruction (offset -4)
            // and if its a match, we need to break in the next instruction (we are in the middle
            // of jump) (+4)
            //
            // Otherwise, we will break on the instruction after the jump (+8)
            if jumping {
                offset = 4;
            } else {
                offset = 0;
            }

            // PC is always word aligned
            let instr = bus.read_u32(regs.pc - offset);
            let instr = Instruction::from_u32(instr, regs.pc);

            // check that the instruction we are about to execute is `Jal/r`
            if let Opcode::Jal | Opcode::Jalr = instr.opcode {
                self.step_over_breakpoints.insert(regs.pc + 8 - offset);
            } else {
                self.step = true;
            }
        }
    }

    pub(crate) fn trace_instruction(
        &mut self,
        regs: &Registers,
        jumping: bool,
        instruction: &Instruction,
    ) -> bool {
        if !self.step_over_breakpoints.is_empty() && self.step_over_breakpoints.contains(&regs.pc) {
            self.step_over_breakpoints.remove(&regs.pc);
            self.set_pause(true);
            self.last_state = CpuState::StepOver;
            return true;
        }

        if !self.in_breakpoint
            && !self.instruction_breakpoints.is_empty()
            && self.instruction_breakpoints.contains(&regs.pc)
        {
            self.in_breakpoint = true;
            self.set_pause(true);
            self.last_state = CpuState::InstructionBreakpoint(regs.pc);
            return true;
        }

        // -- the instruction will execute after this point
        //    i.e. will return `false`

        self.in_breakpoint = false;

        if jumping {
            match self.last_instruction.opcode {
                Opcode::Jal | Opcode::Jalr => {
                    self.call_stack.push(self.last_instruction.pc + 8);
                }
                Opcode::Jr => {
                    // Sometimes, the return address is not always the last on the stack.
                    // For example, when a program calls into the bios with
                    // 0xA0,0xB0,0xC0 functions, an inner function might return
                    // to the user space and not the main handler, which results
                    // in a frame being stuck in the middle.
                    //
                    // That's why we have to check if the return address is any
                    // of the previous frames.
                    let target = regs.read_general(self.last_instruction.rs_raw);

                    if !self.call_stack.is_empty() {
                        let mut c = 1;
                        for x in self.call_stack.iter().rev() {
                            if *x == target {
                                self.call_stack.truncate(self.call_stack.len() - c);
                                break;
                            }

                            c += 1;
                        }
                    }
                }
                _ => {}
            }
        }

        if self.instruction_trace {
            println!(
                "{:08X}: {}{}",
                instruction.pc,
                if jumping { "_" } else { "" },
                instruction
            );
        }

        if self.step {
            self.set_pause(true);
            self.step = false;
            self.last_state = CpuState::Step;
        }

        self.last_instruction = instruction.clone();

        // even if we are in step breakpoint, we must execute the current instruction
        false
    }

    pub(crate) fn trace_write(&mut self, addr: u32, bits: u8) {
        if !self.write_breakpoints.is_empty() && self.write_breakpoints.contains(&addr) {
            self.set_pause(true);
            self.last_state = CpuState::WriteBreakpoint { addr, bits };
        }
    }

    pub(crate) fn trace_read(&mut self, addr: u32, bits: u8) {
        if !self.read_breakpoints.is_empty() && self.read_breakpoints.contains(&addr) {
            self.set_pause(true);
            self.last_state = CpuState::ReadBreakpoint { addr, bits };
        }
    }
}

impl Debugger {
    pub fn single_step(&mut self) {
        self.step = true;
    }

    pub fn step_over(&mut self) {
        self.step_over = true;
    }

    pub fn set_instruction_trace(&mut self, trace: bool) {
        self.instruction_trace = trace;
    }

    pub fn add_breakpoint(&mut self, address: u32) {
        self.instruction_breakpoints.insert(address);
    }

    pub fn remove_breakpoint(&mut self, address: u32) -> bool {
        self.instruction_breakpoints.remove(&address)
    }

    pub fn add_write_breakpoint(&mut self, address: u32) {
        self.write_breakpoints.insert(address);
    }

    pub fn remove_write_breakpoint(&mut self, address: u32) -> bool {
        self.write_breakpoints.remove(&address)
    }

    pub fn add_read_breakpoint(&mut self, address: u32) {
        self.read_breakpoints.insert(address);
    }

    pub fn remove_read_breakpoint(&mut self, address: u32) -> bool {
        self.read_breakpoints.remove(&address)
    }

    pub fn instruction_breakpoints(&self) -> &HashSet<u32> {
        &self.instruction_breakpoints
    }

    pub fn write_breakpoints(&self) -> &HashSet<u32> {
        &self.write_breakpoints
    }

    pub fn read_breakpoints(&self) -> &HashSet<u32> {
        &self.read_breakpoints
    }

    pub fn call_stack(&self) -> &[u32] {
        &self.call_stack
    }
}
