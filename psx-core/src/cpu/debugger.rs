use std::{collections::HashSet, io::Write, process, thread};

use crossbeam::channel::{Receiver, Sender};
use rustyline::{
    completion::Completer, error::ReadlineError, highlight::Highlighter, hint::Hinter,
    history::History, line_buffer::LineBuffer, validate::Validator, CompletionType, Config, Editor,
};

use crate::cpu::register;

use super::{
    instruction::Instruction,
    instruction_format::{ALL_REG_NAMES, GENERAL_REG_NAMES, REG_HI_NAME, REG_LO_NAME, REG_PC_NAME},
    register::{Register, Registers},
    CpuBusProvider,
};

use crate::memory::hw_registers::HW_REGISTERS;

struct EditorHelper {
    hw_registers: Vec<String>,
    cpu_registers: Vec<String>,
}

impl EditorHelper {
    fn new() -> Self {
        Self {
            hw_registers: HW_REGISTERS.keys().map(|name| name.to_string()).collect(),
            cpu_registers: ALL_REG_NAMES.iter().map(|name| name.to_string()).collect(),
        }
    }
}

impl Validator for EditorHelper {}
impl Hinter for EditorHelper {
    type Hint = String;

    fn hint(&self, line: &str, pos: usize, _ctx: &rustyline::Context<'_>) -> Option<Self::Hint> {
        if line.is_empty() || pos < line.len() || !(line.contains('@') || line.contains('$')) {
            return None;
        }

        if let Some(i) = line.rfind(['@', '$']) {
            let reg_type = line[i..].chars().next().unwrap();
            let reg_name = &line[i + 1..];

            let regs = if reg_type == '$' {
                self.cpu_registers.iter()
            } else {
                self.hw_registers.iter()
            };

            regs.filter(|k| k.to_lowercase().starts_with(&reg_name.to_lowercase()))
                .map(|k| k[reg_name.len()..].to_string())
                .next()
        } else {
            None
        }
    }
}
impl Highlighter for EditorHelper {}
impl Completer for EditorHelper {
    type Candidate = String;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &rustyline::Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Self::Candidate>)> {
        if line.is_empty() || !(line.contains('@') || line.contains('$')) {
            return Ok((0, Vec::with_capacity(0)));
        }

        let sub = &line[..pos];

        if let Some(i) = sub.rfind(['@', '$']) {
            let reg_type = sub[i..].chars().next().unwrap();
            let reg_name = &sub[i + 1..];

            let regs = if reg_type == '$' {
                self.cpu_registers.iter()
            } else {
                self.hw_registers.iter()
            };

            let v = regs
                .filter(|k| k.to_lowercase().starts_with(&reg_name.to_lowercase()))
                .map(|k| k.to_string())
                .collect();

            Ok((i + 1, v))
        } else {
            Ok((0, Vec::with_capacity(0)))
        }
    }

    fn update(&self, line: &mut LineBuffer, start: usize, elected: &str) {
        let end = line.pos();
        line.replace(start..end, elected);
    }
}
impl rustyline::Helper for EditorHelper {}

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

fn create_editor() -> Editor<EditorHelper> {
    let conf = Config::builder()
        .auto_add_history(true)
        .completion_type(CompletionType::List)
        .build();
    let mut editor = Editor::with_config(conf).unwrap();
    editor.set_helper(Some(EditorHelper::new()));
    editor
}

pub struct Debugger {
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

    current_pc: u32,
}

impl Debugger {
    pub fn new() -> Self {
        let (stdin_tx, stdin_rx) = crossbeam::channel::bounded(1);
        let (editor_tx, editor_rx) = crossbeam::channel::bounded(1);

        thread::spawn(move || {
            let mut history = History::new();
            let mut editor = None;

            loop {
                if let Ok(cmd) = editor_rx.recv() {
                    match cmd {
                        EditorCmd::Start => {
                            let mut ed = create_editor();
                            // laod history
                            std::mem::swap(ed.history_mut(), &mut history);
                            editor = Some(ed);
                        }
                        EditorCmd::Continue => {
                            assert!(editor.is_some());
                        }
                        EditorCmd::Stop => {
                            let mut ed = editor.take().unwrap();
                            // save history
                            std::mem::swap(ed.history_mut(), &mut history);
                            continue;
                        }
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
            instruction_trace: false,
            paused: false,
            instruction_breakpoints: HashSet::new(),
            write_breakpoints: HashSet::new(),
            in_breakpoint: false,
            step: false,
            stdin_rx,
            editor_tx,

            current_pc: 0,
        }
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

    pub fn paused(&self) -> bool {
        self.paused
    }

    /// Returns true if the CPU should exit and not execute instructions, since
    /// we are paused.
    pub fn handle<P: CpuBusProvider>(&mut self, regs: &Registers, bus: &mut P) -> bool {
        if !self.paused {
            return false;
        }

        if let Ok(cmd) = self.stdin_rx.try_recv() {
            let mut tokens = cmd.split_whitespace();
            let mut cmd = tokens.next();
            let modifier = cmd.and_then(|c| {
                c.split_once('/').map(|(s1, s2)| {
                    cmd = Some(s1);
                    s2
                })
            });
            let addr = tokens.next().and_then(|a| {
                if let Some(register_name) = a.strip_prefix('$') {
                    let register = GENERAL_REG_NAMES
                        .iter()
                        .position(|&r| r == register_name)
                        .map(|i| Register::from_byte(i as u8));
                    match register {
                        Some(r) => Some(regs.read_register(r)),
                        None => match register_name {
                            REG_PC_NAME => Some(regs.pc),
                            REG_HI_NAME => Some(regs.hi),
                            REG_LO_NAME => Some(regs.lo),
                            _ => {
                                println!("Invalid register name: {}", register_name);
                                None
                            }
                        },
                    }
                } else if let Some(hw_register_name) = a.strip_prefix("@") {
                    HW_REGISTERS.get(hw_register_name).copied().or_else(|| {
                        println!("Invalid hardware register name: {}", hw_register_name);
                        None
                    })
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
                    println!("i/[n] [addr] - disassemble instructions");
                }
                Some("r") => regs.debug_print(),
                Some("c") => self.set_pause(false),
                Some("s") => {
                    self.set_pause(false);
                    self.step = true;
                }
                Some("tt") => self.set_instruction_trace(true),
                Some("tf") => self.set_instruction_trace(false),
                Some("stack") => {
                    let n = addr.unwrap_or(10);
                    let sp = regs.read_register(register::RegisterType::Sp.into());
                    println!("Stack: SP=0x{:08X}", sp);
                    for i in 0..n {
                        let d = Self::bus_read_u32(bus, sp + i * 4);
                        if let Some(d) = d {
                            println!("    {:08X}", d);
                        } else {
                            break;
                        }
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
                            let val = Self::bus_read_u32(bus, addr);
                            if let Some(val) = val {
                                println!("0x{:08X}: 0x{:08X}", addr, val);
                            } else {
                                break;
                            }
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
                            let val = Self::bus_read_u16(bus, addr);
                            if let Some(val) = val {
                                println!("0x{:08X}: 0x{:04X}", addr, val);
                            } else {
                                break;
                            }
                        }
                    } else {
                        println!("Usage: m16 <address>");
                    }
                }
                Some("m8") => {
                    let count = modifier.and_then(|m| m.parse::<u32>().ok()).unwrap_or(1);
                    if let Some(addr) = addr {
                        for i in 0..count {
                            let addr = addr + i;
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
                Some("i") | Some("i/") => {
                    let count = modifier.and_then(|m| m.parse::<u32>().ok()).unwrap_or(1);
                    let addr = addr.unwrap_or(regs.pc);

                    let previous_instr_d = Self::bus_read_u32(bus, addr - 4);
                    if let Some(previous_instr_d) = previous_instr_d {
                        let mut previous_instr = Instruction::from_u32(previous_instr_d, addr - 4);

                        for i in 0..count {
                            let addr = addr + i * 4;
                            // will always be aligned
                            let val = Self::bus_read_u32(bus, addr).unwrap();
                            let instr = Instruction::from_u32(val, addr);
                            println!(
                                "0x{:08X}: {}{}",
                                addr,
                                if previous_instr.is_branch() { "_" } else { "" },
                                instr
                            );
                            previous_instr = instr;
                        }
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

        true
    }

    pub fn trace_instruction(&mut self, pc: u32, jumping: bool, instruction: &Instruction) -> bool {
        self.current_pc = pc;

        if !self.in_breakpoint
            && !self.instruction_breakpoints.is_empty()
            && self.instruction_breakpoints.contains(&pc)
        {
            println!("Breakpoint hit at {:08X}", pc);
            self.in_breakpoint = true;
            self.set_pause(true);
            return true;
        }

        self.in_breakpoint = false;

        if self.instruction_trace {
            println!(
                "{:08X}: {}{}",
                pc,
                if jumping { "_" } else { "" },
                instruction
            );
        }

        if self.step {
            self.set_pause(true);
            self.step = false;
        }

        // even if we are in step breakpoint, we must execute the current instruction
        false
    }

    pub fn trace_write(&mut self, addr: u32) {
        if self.write_breakpoints.contains(&addr) {
            let hw_reg_name = HW_REGISTERS
                .entries()
                .find(|(_, &v)| v == addr)
                .map(|(k, _)| format!(" [@{}]", k))
                .unwrap_or("".to_string());

            println!(
                "Write Breakpoint u32 hit {:08X}{} at {:08X}",
                addr, hw_reg_name, self.current_pc
            );
            self.set_pause(true);
        }
    }
}

impl Debugger {
    fn set_instruction_trace(&mut self, trace: bool) {
        self.instruction_trace = trace;
        println!("Instruction trace: {}", self.instruction_trace);
    }

    fn add_breakpoint(&mut self, address: u32) {
        self.instruction_breakpoints.insert(address);
        println!("Breakpoint added: 0x{:08X}", address);
    }

    fn remove_breakpoint(&mut self, address: u32) {
        self.instruction_breakpoints.remove(&address);
        println!("Breakpoint removed: 0x{:08X}", address);
    }

    fn add_write_breakpoint(&mut self, address: u32) {
        self.write_breakpoints.insert(address);
        println!("Write Breakpoint added: 0x{:08X}", address);
    }

    fn remove_write_breakpoint(&mut self, address: u32) {
        self.write_breakpoints.remove(&address);
        println!("Write Breakpoint removed: 0x{:08X}", address);
    }

    fn bus_read_u32<P: CpuBusProvider>(bus: &mut P, addr: u32) -> Option<u32> {
        if addr % 4 != 0 {
            println!("[0x{:08X}]: Address must be aligned to 4 bytes", addr);
            return None;
        }
        Some(bus.read_u32(addr))
    }

    fn bus_read_u16<P: CpuBusProvider>(bus: &mut P, addr: u32) -> Option<u16> {
        if addr % 2 != 0 {
            println!("[0x{:08X}]: Address must be aligned to 2 bytes", addr);
            return None;
        }
        Some(bus.read_u16(addr))
    }
}
