use std::{io::Write, process, sync::mpsc, thread};

use psx_core::{
    cpu::{CpuState, CPU_REGISTERS},
    Psx, HW_REGISTERS,
};
use rustyline::{
    completion::Completer, error::ReadlineError, highlight::Highlighter, hint::Hinter,
    history::MemHistory, line_buffer::LineBuffer, validate::Validator, Changeset, CompletionType,
    Config, Editor,
};

struct EditorHelper {
    hw_registers: Vec<String>,
    cpu_registers: Vec<String>,
}

impl EditorHelper {
    fn new() -> Self {
        Self {
            hw_registers: HW_REGISTERS.keys().map(|name| name.to_string()).collect(),
            cpu_registers: CPU_REGISTERS.keys().map(|name| name.to_string()).collect(),
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
    fn update(&self, line: &mut LineBuffer, start: usize, elected: &str, cl: &mut Changeset) {
        let end = line.pos();
        line.replace(start..end, elected, cl);
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

fn create_editor() -> Editor<EditorHelper, MemHistory> {
    let conf = Config::builder()
        .auto_add_history(true)
        .completion_type(CompletionType::List)
        .build();
    let mut editor = Editor::with_config(conf).unwrap();
    editor.set_helper(Some(EditorHelper::new()));
    editor
}

pub struct Debugger {
    stdin_rx: mpsc::Receiver<String>,
    editor_tx: mpsc::SyncSender<EditorCmd>,
    enabled: bool,
}

impl Debugger {
    pub fn new() -> Self {
        let (stdin_tx, stdin_rx) = mpsc::sync_channel(1);
        let (editor_tx, editor_rx) = mpsc::sync_channel(1);

        thread::spawn(move || {
            let mut history = MemHistory::new();
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
                            drop(ed);
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
            stdin_rx,
            editor_tx,
            enabled: false,
        }
    }

    /// Instructs the editor thread to start reading commands.
    /// Note: make sure you call this function after printing all the output you
    /// need, otherwise the editor thread might print the prompt in between your prints.
    pub fn set_enabled(&mut self, enabled: bool) {
        // only send command to editor thread
        // if we are actually changing the state
        if self.enabled ^ enabled {
            if enabled {
                self.editor_tx.send(EditorCmd::Start).ok();
            } else {
                self.editor_tx.send(EditorCmd::Stop).ok();
            }
        }
        self.enabled = enabled;
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn run(&mut self, psx: &mut Psx) {
        if !self.enabled {
            return;
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
                    let reg_ty = CPU_REGISTERS.get(register_name).copied().or_else(|| {
                        println!("Invalid CPU register name: {}", register_name);
                        None
                    });
                    // read register
                    reg_ty.map(|r| psx.cpu().registers().read(r))
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
                    println!("so - step-over");
                    println!("tt - enable trace");
                    println!("tf - disbale trace");
                    println!("stack [0xn] - print stack [n entries in hex]");
                    println!("bt/[limit] - print backtrace [top `limit` entries]");
                    println!("b <addr> - set breakpoint");
                    println!("rb <addr> - remove breakpoint");
                    println!("bw <addr> - set write breakpoint");
                    println!("rbw <addr> - remove write breakpoint");
                    println!("br <addr> - set read breakpoint");
                    println!("rbr <addr> - remove read breakpoint");
                    println!("lb - list breakpoints");
                    println!("m[32/16/8] <addr> - print content of memory (default u32)");
                    println!("p <addr>/<$reg> - print address or register value");
                    println!("i/[n] [addr] - disassemble instructions");
                }
                Some("r") => println!("{:?}", psx.cpu().registers()),
                Some("c") => {
                    self.set_enabled(false);
                }
                Some("s") => {
                    psx.cpu().debugger().single_step();
                    self.set_enabled(false);
                }
                Some("so") => {
                    psx.cpu().debugger().step_over();
                    self.set_enabled(false);
                }
                Some("tt") => {
                    psx.cpu().debugger().set_instruction_trace(true);
                }
                Some("tf") => {
                    psx.cpu().debugger().set_instruction_trace(false);
                }
                Some("stack") => {
                    //let n = addr.unwrap_or(10);
                    //let sp = regs.read(register::RegisterType::Sp);
                    //println!("Stack: SP=0x{:08X}", sp);
                    //for i in 0..n {
                    //    let d = Self::bus_read_u32(bus, sp + i * 4);
                    //    if let Some(d) = d {
                    //        println!("    {:08X}", d);
                    //    } else {
                    //        break;
                    //    }
                    //}
                }
                Some("bt") => {
                    // let limit = modifier
                    //     .and_then(|m| m.parse::<usize>().ok())
                    //     .unwrap_or(self.call_stack.len());

                    // for (i, frame) in self.call_stack.iter().enumerate().rev().take(limit) {
                    //     println!("#{:02}:      {:08X}", i, frame);
                    // }
                }
                Some("b") => {
                    if let Some(addr) = addr {
                        psx.cpu().debugger().add_breakpoint(addr);
                    } else {
                        println!("Usage: b <address>");
                    }
                }
                Some("rb") => {
                    if let Some(addr) = addr {
                        psx.cpu().debugger().remove_breakpoint(addr);
                    } else {
                        println!("Usage: rb <address>");
                    }
                }
                Some("bw") => {
                    if let Some(addr) = addr {
                        psx.cpu().debugger().add_write_breakpoint(addr);
                    } else {
                        println!("Usage: bw <address>");
                    }
                }
                Some("rbw") => {
                    if let Some(addr) = addr {
                        psx.cpu().debugger().remove_write_breakpoint(addr);
                    } else {
                        println!("Usage: rbw <address>");
                    }
                }
                Some("br") => {
                    if let Some(addr) = addr {
                        psx.cpu().debugger().add_read_breakpoint(addr);
                    } else {
                        println!("Usage: br <address>");
                    }
                }
                Some("rbr") => {
                    if let Some(addr) = addr {
                        psx.cpu().debugger().remove_read_breakpoint(addr);
                    } else {
                        println!("Usage: rbr <address>");
                    }
                }
                Some("lb") => {
                    // for bp in self.instruction_breakpoints.iter() {
                    //     println!("Breakpoint: 0x{:08X}", bp);
                    // }
                    // for bp in self.write_breakpoints.iter() {
                    //     println!("Write Breakpoint: 0x{:08X}", bp);
                    // }
                    // for bp in self.read_breakpoints.iter() {
                    //     println!("Read Breakpoint: 0x{:08X}", bp);
                    // }
                }
                Some("m") | Some("m32") => {
                    // let count = modifier.and_then(|m| m.parse::<u32>().ok()).unwrap_or(1);
                    // if let Some(addr) = addr {
                    //     for i in 0..count {
                    //         let addr = addr + i * 4;
                    //         let val = Self::bus_read_u32(bus, addr);
                    //         if let Some(val) = val {
                    //             println!("0x{:08X}: 0x{:08X}", addr, val);
                    //         } else {
                    //             break;
                    //         }
                    //     }
                    // } else {
                    //     println!("Usage: m/m32 <address>");
                    // }
                }
                Some("m16") => {
                    // let count = modifier.and_then(|m| m.parse::<u32>().ok()).unwrap_or(1);
                    // if let Some(addr) = addr {
                    //     for i in 0..count {
                    //         let addr = addr + i * 2;
                    //         let val = Self::bus_read_u16(bus, addr);
                    //         if let Some(val) = val {
                    //             println!("0x{:08X}: 0x{:04X}", addr, val);
                    //         } else {
                    //             break;
                    //         }
                    //     }
                    // } else {
                    //     println!("Usage: m16 <address>");
                    // }
                }
                Some("m8") => {
                    // let count = modifier.and_then(|m| m.parse::<u32>().ok()).unwrap_or(1);
                    // if let Some(addr) = addr {
                    //     for i in 0..count {
                    //         let addr = addr + i;
                    //         let val = bus.read_u8(addr);
                    //         println!("[0x{:08X}] = 0x{:02X}", addr, val);
                    //     }
                    // } else {
                    //     println!("Usage: m8 <address>");
                    // }
                }
                Some("p") => {
                    if let Some(addr) = addr {
                        println!("0x{:08X}", addr);
                    } else {
                        println!("Usage: p <address>");
                    }
                }
                Some("i") | Some("i/") => {
                    // let count = modifier.and_then(|m| m.parse::<u32>().ok()).unwrap_or(1);
                    // let addr = addr.unwrap_or(regs.pc);

                    // let previous_instr_d = Self::bus_read_u32(bus, addr - 4);
                    // if let Some(previous_instr_d) = previous_instr_d {
                    //     let mut previous_instr = Instruction::from_u32(previous_instr_d, addr - 4);

                    //     for i in 0..count {
                    //         let addr = addr + i * 4;
                    //         // will always be aligned
                    //         let val = Self::bus_read_u32(bus, addr).unwrap();
                    //         let instr = Instruction::from_u32(val, addr);
                    //         println!(
                    //             "0x{:08X}: {}{}",
                    //             addr,
                    //             if previous_instr.is_branch() { "_" } else { "" },
                    //             instr
                    //         );
                    //         previous_instr = instr;
                    //     }
                    // }
                }
                Some("") => {}
                Some(cmd) => println!("Unknown command: {}", cmd),
                _ => (),
            }
            // make sure we send to the editor thread after we printed everything
            // otherwise the editor thread might print the prompt in between
            if self.enabled {
                self.editor_tx.try_send(EditorCmd::Continue).ok();
            }
        }
    }

    pub fn handle_cpu_state(&mut self, _psx: &mut Psx, cpu_state: CpuState) {
        match cpu_state {
            CpuState::Normal => {}
            CpuState::InstructionBreakpoint(addr) => {
                println!("Instruction breakpoint at {:#x}", addr);
                self.set_enabled(true);
            }
            CpuState::WriteBreakpoint { addr, bits } => {
                println!("Write breakpoint at {:#x} with bits {:#x}", addr, bits);
                self.set_enabled(true);
            }
            CpuState::ReadBreakpoint { addr, bits } => {
                println!("Read breakpoint at {:#x} with bits {:#x}", addr, bits);
                self.set_enabled(true);
            }
            CpuState::Step => {
                self.set_enabled(true);
            }
            CpuState::StepOver => {
                self.set_enabled(true);
            }
        }
    }
}
