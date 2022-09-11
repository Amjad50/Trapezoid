mod instruction;
mod instruction_format;
mod instructions_table;
mod register;

use std::collections::HashSet;
use std::io::Write;
use std::{process, thread};

use crate::coprocessor::{Gte, SystemControlCoprocessor};
use crate::memory::BusLine;

use crossbeam::channel::{Receiver, Sender};
use instruction::{Instruction, Opcode};
use register::Registers;
use rustyline::error::ReadlineError;
use rustyline::{Config, Editor};

use self::instruction_format::{GENERAL_REG_NAMES, REG_HI_NAME, REG_LO_NAME, REG_PC_NAME};
use self::register::Register;

/// Instructing the editor thread on what to do.
///
/// The reason for this, is that the `Editor` object transforms the terminal
/// to raw mode, and thus if its created, we can't Ctrl-C to kill the emulator.
/// For better user experience, we create the editor only when needed.
enum EditorCmd {
    /// Create a new editor and read input
    Start,
    /// Continue reading input
    Continue,
    /// Stop reading input and destroy the editor
    Stop,
}

fn create_editor() -> Editor<()> {
    let conf = Config::builder().auto_add_history(true).build();
    Editor::with_config(conf).unwrap()
}

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

pub struct Cpu {
    regs: Registers,
    cop0: SystemControlCoprocessor,
    cop2: Gte,

    jump_dest_next: Option<u32>,

    elapsed_cycles: u32,

    instruction_trace: bool,
    paused: bool,
    instruction_breakpoints: HashSet<u32>,
    write_breakpoints: HashSet<u32>,
    // currently on top of breakpoint, so ignore it and continue when unpaused
    // so that we don't get stuck in one instruction.
    in_breakpoint: bool,
    // allow to execute one instruction only
    step: bool,
    stdin_rx: Receiver<String>,
    editor_tx: Sender<EditorCmd>,
}

impl Cpu {
    pub fn new() -> Self {
        let (stdin_tx, stdin_rx) = crossbeam::channel::bounded(1);
        let (editor_tx, editor_rx) = crossbeam::channel::bounded(1);

        thread::spawn(move || {
            let mut editor = None;

            loop {
                if let Ok(cmd) = editor_rx.recv() {
                    match cmd {
                        EditorCmd::Start => editor = Some(create_editor()),
                        EditorCmd::Continue => {
                            assert!(editor.is_some());
                        }
                        EditorCmd::Stop => continue,
                    }
                    // flush all outputs
                    std::io::stdout().flush().unwrap();
                    match editor.as_mut().unwrap().readline("CPU> ") {
                        Ok(line) => {
                            stdin_tx.send(line).unwrap();
                        }
                        Err(ReadlineError::Interrupted) => process::exit(0),
                        _ => {}
                    }
                }
            }
        });

        Self {
            // reset value
            regs: Registers::new(),
            cop0: SystemControlCoprocessor::default(),
            cop2: Gte::default(),
            jump_dest_next: None,

            elapsed_cycles: 0,

            instruction_trace: false,
            paused: false,
            instruction_breakpoints: HashSet::new(),
            write_breakpoints: HashSet::new(),
            in_breakpoint: false,
            step: false,
            stdin_rx,
            editor_tx,
        }
    }

    pub fn set_instruction_trace(&mut self, trace: bool) {
        self.instruction_trace = trace;
        println!("Instruction trace: {}", self.instruction_trace);
    }

    /// Pause the CPU and instructs the editor thread to start reading commands.
    /// Note: make sure you call this function after printing all the output you
    /// need, otherwise the editor thread might print the prompt in between your prints.
    pub fn set_pause(&mut self, pause: bool) {
        // only send command to editor thread
        // if we are actually changing the state
        if self.paused ^ pause {
            if pause {
                self.editor_tx.send(EditorCmd::Start).ok();
            } else {
                self.editor_tx.send(EditorCmd::Stop).ok();
            }
        }

        self.paused = pause;
    }

    pub fn add_breakpoint(&mut self, address: u32) {
        self.instruction_breakpoints.insert(address);
        println!("Breakpoint added: 0x{:08X}", address);
    }

    pub fn remove_breakpoint(&mut self, address: u32) {
        self.instruction_breakpoints.remove(&address);
        println!("Breakpoint removed: 0x{:08X}", address);
    }

    pub fn add_write_breakpoint(&mut self, address: u32) {
        self.write_breakpoints.insert(address);
        println!("Write Breakpoint added: 0x{:08X}", address);
    }

    pub fn remove_write_breakpoint(&mut self, address: u32) {
        self.write_breakpoints.remove(&address);
        println!("Write Breakpoint removed: 0x{:08X}", address);
    }

    pub fn print_cpu_registers(&self) {
        self.regs.debug_print();
    }

    pub fn clock<P: CpuBusProvider>(&mut self, bus: &mut P, clocks: u32) -> u32 {
        if self.paused {
            if let Ok(cmd) = self.stdin_rx.try_recv() {
                let mut tokens = cmd.trim().split_whitespace();
                let mut cmd = tokens.next();
                let modifier = cmd.and_then(|c| {
                    c.split_once('/').map(|(s1, s2)| {
                        cmd = Some(s1);
                        s2
                    })
                });
                let addr = tokens.next().and_then(|a| {
                    if a.starts_with('$') {
                        let register_name = &a[1..];
                        let register = GENERAL_REG_NAMES
                            .iter()
                            .position(|&r| r == register_name)
                            .map(|i| Register::from_byte(i as u8));
                        match register {
                            Some(r) => Some(self.regs.read_register(r)),
                            None => match register_name {
                                REG_PC_NAME => Some(self.regs.pc),
                                REG_HI_NAME => Some(self.regs.hi),
                                REG_LO_NAME => Some(self.regs.lo),
                                _ => {
                                    println!("Invalid register name: {}", register_name);
                                    None
                                }
                            },
                        }
                    } else {
                        let value = u32::from_str_radix(a.trim_start_matches("0x"), 16);
                        match value {
                            Ok(value) => Some(value),
                            Err(_) => {
                                println!("Invalid address: {}", a);
                                None
                            }
                        }
                    }
                });

                match cmd {
                    Some("h") => {
                        println!("h - help");
                        println!("r - print registers");
                        println!("c - continue");
                        println!("s - step");
                        println!("tt - enable trace");
                        println!("tf - disbale trace");
                        println!("stack [0xn] - print stack [n entries in hex]");
                        println!("b <addr> - set breakpoint");
                        println!("rb <addr> - remove breakpoint");
                        println!("wb <addr> - set write breakpoint");
                        println!("wrb <addr> - remove write breakpoint");
                        println!("lb - list breakpoints");
                        println!("m[32/16/8] <addr> - print content of memory (default u32)");
                        println!("p <addr>/<$reg> - print address or register value");
                    }
                    Some("r") => self.print_cpu_registers(),
                    Some("c") => self.set_pause(false),
                    Some("s") => {
                        self.set_pause(false);
                        self.step = true;
                    }
                    Some("tt") => self.set_instruction_trace(true),
                    Some("tf") => self.set_instruction_trace(false),
                    Some("stack") => {
                        let n = addr.unwrap_or(10);
                        let sp = self.regs.read_register(register::RegisterType::Sp.into());
                        println!("Stack: SP=0x{:08X}", sp);
                        for i in 0..n {
                            println!("    {:08X}", bus.read_u32(sp + i * 4));
                        }
                    }
                    Some("b") => {
                        if let Some(addr) = addr {
                            self.add_breakpoint(addr);
                        } else {
                            println!("Usage: b <address>");
                        }
                    }
                    Some("rb") => {
                        if let Some(addr) = addr {
                            self.remove_breakpoint(addr);
                        } else {
                            println!("Usage: rb <address>");
                        }
                    }
                    Some("wb") => {
                        if let Some(addr) = addr {
                            self.add_write_breakpoint(addr);
                        } else {
                            println!("Usage: wb <address>");
                        }
                    }
                    Some("wrb") => {
                        if let Some(addr) = addr {
                            self.remove_write_breakpoint(addr);
                        } else {
                            println!("Usage: wrb <address>");
                        }
                    }
                    Some("lb") => {
                        for bp in self.instruction_breakpoints.iter() {
                            println!("Breakpoint: 0x{:08X}", bp);
                        }
                        for bp in self.write_breakpoints.iter() {
                            println!("Write Breakpoint: 0x{:08X}", bp);
                        }
                    }
                    Some("m") | Some("m32") => {
                        let count = modifier.and_then(|m| m.parse::<u32>().ok()).unwrap_or(1);
                        if let Some(addr) = addr {
                            for i in 0..count {
                                let addr = addr + i * 4;
                                let val = bus.read_u32(addr);
                                println!("[0x{:08X}] = 0x{:08X}", addr, val);
                            }
                        } else {
                            println!("Usage: m/m32 <address>");
                        }
                    }
                    Some("m16") => {
                        let count = modifier.and_then(|m| m.parse::<u32>().ok()).unwrap_or(1);
                        if let Some(addr) = addr {
                            for i in 0..count {
                                let addr = addr + i * 2;
                                let val = bus.read_u16(addr);
                                println!("[0x{:08X}] = 0x{:04X}", addr, val);
                            }
                        } else {
                            println!("Usage: m16 <address>");
                        }
                    }
                    Some("m8") => {
                        let count = modifier.and_then(|m| m.parse::<u32>().ok()).unwrap_or(1);
                        if let Some(addr) = addr {
                            for i in 0..count {
                                let addr = addr + i * 1;
                                let val = bus.read_u8(addr);
                                println!("[0x{:08X}] = 0x{:02X}", addr, val);
                            }
                        } else {
                            println!("Usage: m8 <address>");
                        }
                    }
                    Some("p") => {
                        if let Some(addr) = addr {
                            println!("0x{:08X}", addr);
                        } else {
                            println!("Usage: p <address>");
                        }
                    }
                    Some("") => {}
                    Some(cmd) => println!("Unknown command: {}", cmd),
                    _ => (),
                }
                // make sure we send to the editor thread after we printed everything
                // otherwise the editor thread might print the prompt in between
                if self.paused {
                    self.editor_tx.try_send(EditorCmd::Continue).ok();
                }
            }

            return 0;
        }

        let pending_interrupts = bus.pending_interrupts();
        self.check_and_execute_interrupt(pending_interrupts);

        for _ in 0..clocks {
            if !self.in_breakpoint
                && !self.instruction_breakpoints.is_empty()
                && self.instruction_breakpoints.contains(&self.regs.pc)
            {
                println!("Breakpoint hit at {:08X}", self.regs.pc);
                self.in_breakpoint = true;
                self.set_pause(true);
                break;
            }
            self.in_breakpoint = false;

            if let Some(instruction) = self.bus_read_u32(bus, self.regs.pc) {
                let instruction = Instruction::from_u32(instruction);

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
                if self.instruction_trace {
                    println!(
                        "{:08X}: {}{}",
                        self.regs.pc,
                        if self.jump_dest_next.is_some() {
                            "_"
                        } else {
                            ""
                        },
                        instruction
                    );
                }
                self.regs.pc += 4;
                if let Some(jump_dest) = self.jump_dest_next.take() {
                    log::trace!("pc jump {:08X}", jump_dest);
                    self.regs.pc = jump_dest;
                }

                self.execute_instruction(&instruction, bus);

                if self.step {
                    self.set_pause(true);
                    self.step = false;
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

                // can be set with write breakpoint
                if self.paused {
                    break;
                }
            }
        }

        std::mem::take(&mut self.elapsed_cycles)
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
        let rs = self.regs.read_register(instruction.rs);
        let rt = self.regs.read_register(instruction.rt);

        let (result, overflow) = handler(rs, rt);
        if overflow {
            self.execute_exception(Exception::ArithmeticOverflow);
        } else {
            self.regs.write_register(instruction.rd, result);
        }
    }

    #[inline]
    fn execute_alu_imm<F>(&mut self, instruction: &Instruction, handler: F)
    where
        F: FnOnce(u32, &Instruction) -> u32,
    {
        let rs = self.regs.read_register(instruction.rs);
        let result = handler(rs, &instruction);
        self.regs.write_register(instruction.rt, result);
    }

    #[inline]
    fn execute_branch<F>(&mut self, instruction: &Instruction, have_rt: bool, handler: F)
    where
        F: FnOnce(i32, i32) -> bool,
    {
        let rs = self.regs.read_register(instruction.rs) as i32;
        let rt = if have_rt {
            self.regs.read_register(instruction.rt) as i32
        } else {
            0
        };
        let signed_imm16 = Self::sign_extend_16(instruction.imm16).wrapping_mul(4);

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
        let rs = self.regs.read_register(instruction.rs);
        let computed_addr = rs.wrapping_add(Self::sign_extend_16(instruction.imm16));

        if let Some(data) = handler(self, computed_addr) {
            self.regs.write_register(instruction.rt, data);
        }
    }

    #[inline]
    fn execute_store<F>(&mut self, instruction: &Instruction, mut handler: F)
    where
        F: FnMut(&mut Self, u32, u32),
    {
        let rs = self.regs.read_register(instruction.rs);
        let rt = self.regs.read_register(instruction.rt);
        let computed_addr = rs.wrapping_add(Self::sign_extend_16(instruction.imm16));

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
                let rs = self.regs.read_register(instruction.rs);
                let computed_addr = rs.wrapping_add(Self::sign_extend_16(instruction.imm16));

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
                let original_rt = self.regs.read_register(instruction.rt);
                let result = (original_rt & mask) | result;

                self.regs.write_register(instruction.rt, result);
            }
            Opcode::Lwr => {
                let rs = self.regs.read_register(instruction.rs);
                let computed_addr = rs.wrapping_add(Self::sign_extend_16(instruction.imm16));

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
                let original_rt = self.regs.read_register(instruction.rt);
                let result = (original_rt & mask) | result;

                self.regs.write_register(instruction.rt, result);
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
                let rs = self.regs.read_register(instruction.rs);
                let mut rt = self.regs.read_register(instruction.rt);
                let computed_addr = rs.wrapping_add(Self::sign_extend_16(instruction.imm16));

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
                let rs = self.regs.read_register(instruction.rs);
                let mut rt = self.regs.read_register(instruction.rt);
                let computed_addr = rs.wrapping_add(Self::sign_extend_16(instruction.imm16));

                let start = computed_addr;
                let end = computed_addr | 3;

                // write the data in little endian
                for part_addr in start..=end {
                    self.bus_write_u8(bus, part_addr, rt as u8);
                    rt >>= 8;
                }
            }
            Opcode::Slt => {
                let rs = self.regs.read_register(instruction.rs) as i32;
                let rt = self.regs.read_register(instruction.rt) as i32;

                self.regs.write_register(instruction.rd, (rs < rt) as u32);
            }
            Opcode::Sltu => {
                let rs = self.regs.read_register(instruction.rs);
                let rt = self.regs.read_register(instruction.rt);

                self.regs.write_register(instruction.rd, (rs < rt) as u32);
            }
            Opcode::Slti => {
                let rs = self.regs.read_register(instruction.rs) as i32;
                let imm = instruction.imm16 as i16 as i32;

                self.regs.write_register(instruction.rt, (rs < imm) as u32);
            }
            Opcode::Sltiu => {
                let rs = self.regs.read_register(instruction.rs);
                let imm = Self::sign_extend_16(instruction.imm16);

                self.regs.write_register(instruction.rt, (rs < imm) as u32);
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
                    rs.wrapping_add(Self::sign_extend_16(instr.imm16))
                });
            }
            Opcode::Addi => {
                let rs = self.regs.read_register(instruction.rs);
                let (result, overflow) =
                    (rs as i32).overflowing_add(Self::sign_extend_16(instruction.imm16) as i32);

                if overflow {
                    self.execute_exception(Exception::ArithmeticOverflow);
                } else {
                    self.regs.write_register(instruction.rt, result as u32);
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
                self.execute_alu_imm(instruction, |rs, instr| rs & (instr.imm16 as u32));
            }
            Opcode::Ori => {
                self.execute_alu_imm(instruction, |rs, instr| rs | (instr.imm16 as u32));
            }
            Opcode::Xori => {
                self.execute_alu_imm(instruction, |rs, instr| rs ^ (instr.imm16 as u32));
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
                let rt = self.regs.read_register(instruction.rt);
                let result = rt << instruction.imm5;
                self.regs.write_register(instruction.rd, result);
            }
            Opcode::Srl => {
                let rt = self.regs.read_register(instruction.rt);
                let result = rt >> instruction.imm5;
                self.regs.write_register(instruction.rd, result);
            }
            Opcode::Sra => {
                let rt = self.regs.read_register(instruction.rt);
                let result = ((rt as i32) >> instruction.imm5) as u32;
                self.regs.write_register(instruction.rd, result);
            }
            Opcode::Lui => {
                let result = (instruction.imm16 as u32) << 16;
                self.regs.write_register(instruction.rt, result);
            }
            Opcode::Mult => {
                let rs = self.regs.read_register(instruction.rs) as i32 as i64;
                let rt = self.regs.read_register(instruction.rt) as i32 as i64;

                let result = (rs * rt) as u64;

                self.regs.hi = (result >> 32) as u32;
                self.regs.lo = result as u32;
            }
            Opcode::Multu => {
                let rs = self.regs.read_register(instruction.rs) as u64;
                let rt = self.regs.read_register(instruction.rt) as u64;

                let result = rs * rt;

                self.regs.hi = (result >> 32) as u32;
                self.regs.lo = result as u32;
            }
            Opcode::Div => {
                let rs = self.regs.read_register(instruction.rs) as i32 as i64;
                let rt = self.regs.read_register(instruction.rt) as i32 as i64;

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
                let rs = self.regs.read_register(instruction.rs) as u64;
                let rt = self.regs.read_register(instruction.rt) as u64;

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
                self.regs.write_register(instruction.rd, self.regs.hi);
            }
            Opcode::Mthi => {
                self.regs.hi = self.regs.read_register(instruction.rs);
            }
            Opcode::Mflo => {
                self.regs.write_register(instruction.rd, self.regs.lo);
            }
            Opcode::Mtlo => {
                self.regs.lo = self.regs.read_register(instruction.rs);
            }
            Opcode::J => {
                let base = self.regs.pc & 0xF0000000;
                let offset = instruction.imm26 * 4;

                self.jump_dest_next = Some(base + offset);
            }
            Opcode::Jal => {
                let base = self.regs.pc & 0xF0000000;
                let offset = instruction.imm26 * 4;

                self.jump_dest_next = Some(base + offset);

                self.regs.write_ra(self.regs.pc + 4);
            }
            Opcode::Jr => {
                self.jump_dest_next = Some(self.regs.read_register(instruction.rs));
            }
            Opcode::Jalr => {
                self.jump_dest_next = Some(self.regs.read_register(instruction.rs));

                self.regs.write_register(instruction.rd, self.regs.pc + 4);
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

                self.cop2.execute_command(instruction.imm25);
            }
            Opcode::Mfc(n) => {
                let result = match n {
                    0 => self.cop0.read_data(instruction.rd_raw),
                    2 => self.cop2.read_data(instruction.rd_raw),
                    _ => unreachable!(),
                };

                self.regs.write_register(instruction.rt, result);
            }
            Opcode::Cfc(n) => {
                let result = match n {
                    0 => self.cop0.read_ctrl(instruction.rd_raw),
                    2 => self.cop2.read_ctrl(instruction.rd_raw),
                    _ => unreachable!(),
                };

                self.regs.write_register(instruction.rt, result);
            }
            Opcode::Mtc(n) => {
                let rt = self.regs.read_register(instruction.rt);

                match n {
                    0 => self.cop0.write_data(instruction.rd_raw, rt),
                    2 => self.cop2.write_data(instruction.rd_raw, rt),
                    _ => unreachable!(),
                }
            }
            Opcode::Ctc(n) => {
                let rt = self.regs.read_register(instruction.rt);

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
                let rs = self.regs.read_register(instruction.rs);
                let computed_addr = rs.wrapping_add(Self::sign_extend_16(instruction.imm16));

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
            Opcode::Special => unreachable!(),
            _ => todo!("unimplemented_instruction {:?}", instruction.opcode),
        }
    }
}

impl Cpu {
    fn bus_read_u32<P: BusLine>(&mut self, bus: &mut P, addr: u32) -> Option<u32> {
        self.elapsed_cycles += 1;

        if addr % 4 != 0 {
            self.execute_exception(Exception::AddressErrorLoad);
            self.cop0.write_bad_vaddr(addr);

            return None;
        }

        Some(match addr {
            0x00000000..=0x00001000 if self.cop0.is_cache_isolated() => 0,
            _ => bus.read_u32(addr),
        })
    }

    fn bus_write_u32<P: BusLine>(&mut self, bus: &mut P, addr: u32, data: u32) {
        self.elapsed_cycles += 1;

        if addr % 4 != 0 {
            self.execute_exception(Exception::AddressErrorStore);
            self.cop0.write_bad_vaddr(addr);
        } else {
            if self.write_breakpoints.contains(&addr) {
                println!(
                    "Write Breakpoint u32 hit {:08X} at {:08X}",
                    addr, self.regs.pc
                );
                self.set_pause(true);
            }
            match addr {
                0x00000000..=0x00001000 if self.cop0.is_cache_isolated() => {}
                _ => bus.write_u32(addr, data),
            }
        }
    }

    fn bus_read_u16<P: BusLine>(&mut self, bus: &mut P, addr: u32) -> Option<u16> {
        if addr % 2 != 0 {
            self.execute_exception(Exception::AddressErrorLoad);
            self.cop0.write_bad_vaddr(addr);

            return None;
        }

        Some(match addr {
            0x00000000..=0x00001000 if self.cop0.is_cache_isolated() => 0,
            _ => bus.read_u16(addr),
        })
    }

    fn bus_write_u16<P: BusLine>(&mut self, bus: &mut P, addr: u32, data: u16) {
        if addr % 2 != 0 {
            self.execute_exception(Exception::AddressErrorStore);
            self.cop0.write_bad_vaddr(addr);
        } else {
            if self.write_breakpoints.contains(&addr) {
                println!(
                    "Write Breakpoint u16 hit {:08X} at {:08X}",
                    addr, self.regs.pc
                );
                self.set_pause(true);
            }
            match addr {
                0x00000000..=0x00001000 if self.cop0.is_cache_isolated() => {}
                _ => bus.write_u16(addr, data),
            }
        }
    }

    fn bus_read_u8<P: BusLine>(&self, bus: &mut P, addr: u32) -> u8 {
        match addr {
            0x00000000..=0x00001000 if self.cop0.is_cache_isolated() => 0,
            _ => bus.read_u8(addr),
        }
    }

    fn bus_write_u8<P: BusLine>(&mut self, bus: &mut P, addr: u32, data: u8) {
        if self.write_breakpoints.contains(&addr) {
            // TODO: fix the `pc` here
            println!(
                "Write Breakpoint u8 hit {:08X} at {:08X}",
                addr, self.regs.pc
            );
            self.set_pause(true);
        }
        match addr {
            0x00000000..=0x00001000 if self.cop0.is_cache_isolated() => {}
            _ => bus.write_u8(addr, data),
        }
    }
}
