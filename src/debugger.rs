use std::{io::Write, process, sync::mpsc, thread};

use rustyline::{
    completion::Completer, error::ReadlineError, highlight::Highlighter, hint::Hinter,
    history::MemHistory, line_buffer::LineBuffer, validate::Validator, Changeset, CompletionType,
    Config, Editor,
};
use trapezoid_core::{
    cpu::{CpuState, Instruction, RegisterType, CPU_REGISTERS},
    Psx, HW_REGISTERS,
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

struct RunHookSettings {
    step: bool,
    step_over: bool,
    step_out: bool,
    instruction_breakpoint: bool,
    read_breakpoint: bool,
    write_breakpoint: bool,
}

pub struct Debugger {
    stdin_rx: mpsc::Receiver<String>,
    editor_tx: mpsc::SyncSender<EditorCmd>,
    enabled: bool,
    breakpoint_hooks: Vec<String>,
    run_hook_settings: RunHookSettings,
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
                        EditorCmd::Continue => {}
                        EditorCmd::Stop => {
                            if editor.is_none() {
                                continue;
                            }
                            let mut ed = editor.take().unwrap();
                            // save history
                            std::mem::swap(ed.history_mut(), &mut history);
                            drop(ed);
                            continue;
                        }
                    }
                    // flush all outputs
                    std::io::stdout().flush().unwrap();
                    if let Some(editor) = &mut editor {
                        match editor.readline("CPU> ") {
                            Ok(line) => {
                                stdin_tx.send(line).unwrap();
                            }
                            Err(ReadlineError::Interrupted) => process::exit(0),
                            _ => {}
                        }
                    }
                }
            }
        });

        Self {
            stdin_rx,
            editor_tx,
            enabled: false,
            breakpoint_hooks: Vec::new(),
            run_hook_settings: RunHookSettings {
                step: false,
                step_over: false,
                step_out: false,
                instruction_breakpoint: false,
                read_breakpoint: false,
                write_breakpoint: false,
            },
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
                self.editor_tx.try_send(EditorCmd::Start).ok();
            } else {
                self.editor_tx.try_send(EditorCmd::Stop).ok();
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
            self.handle_command(psx, &cmd);

            // make sure we send to the editor thread after we printed everything
            // otherwise the editor thread might print the prompt in between
            if self.enabled {
                self.editor_tx.try_send(EditorCmd::Continue).ok();
            }
        }
    }

    fn handle_command(&mut self, psx: &mut Psx, cmd: &str) {
        fn parse_register_name(name: &str) -> Option<RegisterType> {
            CPU_REGISTERS.get(name).copied().or_else(|| {
                println!("Invalid CPU register name: {}", name);
                None
            })
        }

        fn parse_address(a: &str, psx: &mut Psx) -> Option<u32> {
            if let Some(register_name) = a.strip_prefix('$') {
                let reg_ty = parse_register_name(register_name);
                reg_ty.map(|r| psx.cpu().registers().read(r))
            } else if let Some(hw_register_name) = a.strip_prefix('@') {
                HW_REGISTERS.get(hw_register_name).copied().or_else(|| {
                    println!("Invalid hardware register name: {}", hw_register_name);
                    None
                })
            } else {
                let value = u32::from_str_radix(a.trim_start_matches("0x"), 16);
                match value {
                    Ok(value) => Some(value),
                    Err(_) => None,
                }
            }
        }

        let (mut cmd, arg) = match cmd.trim().split_once(' ') {
            Some((c, a)) => (c, Some(a)),
            None => (cmd, None),
        };
        let modifier = cmd.split_once('/').map(|(s1, s2)| {
            cmd = s1;
            s2
        });
        let addr = arg.and_then(|a| {
            if cmd != "set" {
                parse_address(a, psx)
            } else {
                None
            }
        });

        match cmd {
            "h" => {
                println!("h - help");
                println!("reset - reset the game and reboot");
                println!("r - print registers");
                println!("c - continue");
                println!("s - step");
                println!("so - step-over");
                println!("su - step-out");
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
                println!("md/[n] <addr> - memory dump ([n] argument will print the next multiple of 16 after n)");
                println!("p <addr>/<$reg> - print address or register value");
                println!("set <$reg> <value> - set register value (if it can be modified)");
                println!("i/[n] [addr] - disassemble instructions");
                println!("spu - print SPU state");
                println!("hook_add <cmd[;cmd]> - add hook/s commands");
                println!("hook_clear - clear all hooks");
                println!("hook_list - list all hooks");
                println!(
                    "hook_setting [<break_type>[=true/false]] - change when the hooks are executed"
                );
            }
            "reset" => {
                psx.reset();
                println!("Reset");
            }
            "r" => println!("{:?}", psx.cpu().registers()),
            "c" => {
                self.set_enabled(false);
            }
            "s" => {
                psx.cpu().debugger().single_step();
                self.set_enabled(false);
            }
            "so" => {
                psx.cpu().debugger().step_over();
                self.set_enabled(false);
            }
            "su" => {
                psx.cpu().debugger().step_out();
                self.set_enabled(false);
            }
            "tt" => {
                psx.cpu()
                    .debugger()
                    .set_instruction_trace_handler(Some(Box::new(
                        |_regs, instruction, jumping| {
                            println!(
                                "{:08X}: {}{}",
                                instruction.pc,
                                if jumping { "_" } else { "" },
                                instruction
                            );
                        },
                    )));
                println!("Instruction trace: true");
            }
            "tf" => {
                psx.cpu().debugger().set_instruction_trace_handler(None);
                println!("Instruction trace: false");
            }
            "stack" => {
                let n = addr.unwrap_or(10);
                let sp = psx.cpu().registers().read(RegisterType::Sp);
                println!("Stack: SP=0x{:08X}", sp);
                for i in 0..n {
                    let d = psx.bus_read_u32(sp + i * 4);
                    if let Ok(d) = d {
                        println!("    {:08X}", d);
                    } else {
                        println!("    Error reading {:08X}: {:?}", sp + i * 4, d);
                        break;
                    }
                }
            }
            "bt" => {
                let call_stack = psx.cpu().debugger().call_stack();
                let limit = modifier
                    .and_then(|m| m.parse::<usize>().ok())
                    .unwrap_or(call_stack.len());

                for (i, frame) in call_stack.iter().enumerate().rev().take(limit) {
                    println!("#{:02}:      {:08X}", i, frame);
                }
            }
            "b" => {
                if let Some(addr) = addr {
                    psx.cpu().debugger().add_breakpoint(addr);
                    println!("Breakpoint added: 0x{:08X}", addr);
                } else {
                    println!("Usage: b <address>");
                }
            }
            "rb" => {
                if let Some(addr) = addr {
                    if psx.cpu().debugger().remove_breakpoint(addr) {
                        println!("Breakpoint removed: 0x{:08X}", addr);
                    } else {
                        println!("Breakpoint not found: 0x{:08X}", addr);
                    }
                } else {
                    println!("Usage: rb <address>");
                }
            }
            "bw" => {
                if let Some(addr) = addr {
                    psx.cpu().debugger().add_write_breakpoint(addr);
                    println!("Write Breakpoint added: 0x{:08X}", addr);
                } else {
                    println!("Usage: bw <address>");
                }
            }
            "rbw" => {
                if let Some(addr) = addr {
                    if psx.cpu().debugger().remove_write_breakpoint(addr) {
                        println!("Write Breakpoint removed: 0x{:08X}", addr);
                    } else {
                        println!("Write Breakpoint not found: 0x{:08X}", addr);
                    }
                } else {
                    println!("Usage: rbw <address>");
                }
            }
            "br" => {
                if let Some(addr) = addr {
                    psx.cpu().debugger().add_read_breakpoint(addr);
                    println!("Read Breakpoint added: 0x{:08X}", addr);
                } else {
                    println!("Usage: br <address>");
                }
            }
            "rbr" => {
                if let Some(addr) = addr {
                    if psx.cpu().debugger().remove_read_breakpoint(addr) {
                        println!("Read Breakpoint removed: 0x{:08X}", addr);
                    } else {
                        println!("Read Breakpoint not found: 0x{:08X}", addr);
                    }
                } else {
                    println!("Usage: rbr <address>");
                }
            }
            "lb" => {
                for bp in psx.cpu().debugger().instruction_breakpoints().iter() {
                    println!("Breakpoint: 0x{:08X}", bp);
                }
                for bp in psx.cpu().debugger().write_breakpoints().iter() {
                    println!("Write Breakpoint: 0x{:08X}", bp);
                }
                for bp in psx.cpu().debugger().read_breakpoints().iter() {
                    println!("Read Breakpoint: 0x{:08X}", bp);
                }
            }
            "md" => {
                let count = modifier.and_then(|m| m.parse::<u32>().ok()).unwrap_or(1);
                let addr = addr.unwrap_or(psx.cpu().registers().read(RegisterType::Pc));
                let rows = (count + 15) / 16;
                // print in hex dump
                for i in 0..rows {
                    let addr = addr + i * 16;
                    let mut line = format!("{:08X}: ", addr);
                    for j in 0..16 {
                        let val = psx.bus_read_u8(addr + j);
                        if let Ok(val) = val {
                            line.push_str(&format!("{:02X} ", val));
                        } else {
                            line.push_str("?? ");
                        }
                    }
                    println!("{}", line);
                }
            }
            "m" | "m32" => {
                let count = modifier.and_then(|m| m.parse::<u32>().ok()).unwrap_or(1);
                if let Some(addr) = addr {
                    for i in 0..count {
                        let addr = addr + i * 4;
                        let val = psx.bus_read_u32(addr);
                        if let Ok(val) = val {
                            println!("0x{:08X}: 0x{:08X}", addr, val);
                        } else {
                            println!("Error reading u32 {:08X}: {:?}", addr, val);
                            break;
                        }
                    }
                } else {
                    println!("Usage: m/m32 <address>");
                }
            }
            "m16" => {
                let count = modifier.and_then(|m| m.parse::<u32>().ok()).unwrap_or(1);
                if let Some(addr) = addr {
                    for i in 0..count {
                        let addr = addr + i * 2;
                        let val = psx.bus_read_u16(addr);
                        if let Ok(val) = val {
                            println!("0x{:08X}: 0x{:04X}", addr, val);
                        } else {
                            println!("Error reading u16 {:08X}: {:?}", addr, val);
                            break;
                        }
                    }
                } else {
                    println!("Usage: m16 <address>");
                }
            }
            "m8" => {
                let count = modifier.and_then(|m| m.parse::<u32>().ok()).unwrap_or(1);
                if let Some(addr) = addr {
                    for i in 0..count {
                        let addr = addr + i;
                        let val = psx.bus_read_u8(addr).unwrap();
                        println!("0x{:08X}: 0x{:02X}", addr, val);
                    }
                } else {
                    println!("Usage: m8 <address>");
                }
            }
            "p" => {
                if let Some(addr) = addr {
                    println!("0x{:08X}", addr);
                } else {
                    println!("Usage: p <address>");
                }
            }
            "set" => {
                let Some(arg) = arg else {
                    println!("Usage: set <$reg> <value>");
                    return;
                };

                let Some((reg, value)) = arg.split_once(' ') else {
                    println!("Usage: set <$reg> <value>");
                    return;
                };

                let Some(register_name) = reg.strip_prefix('$') else {
                    println!("Invalid register name: {}", reg);
                    return;
                };
                let Some(reg_ty) = parse_register_name(register_name) else {
                    println!("Invalid register name: {}", register_name);
                    return;
                };
                let Some(value) = parse_address(value, psx) else {
                    println!("Invalid value: {}", value);
                    return;
                };

                println!("Set register {} to 0x{:08X}", reg_ty, value);
                psx.cpu().registers_mut().write(reg_ty, value);
            }
            "i" | "i/" => {
                let count = modifier.and_then(|m| m.parse::<u32>().ok()).unwrap_or(1);
                let addr = addr.unwrap_or(psx.cpu().registers().read(RegisterType::Pc));

                let previous_instr_d = psx.bus_read_u32(addr - 4);
                if let Ok(previous_instr_d) = previous_instr_d {
                    let mut previous_instr = Instruction::from_u32(previous_instr_d, addr - 4);

                    for i in 0..count {
                        let addr = addr + i * 4;
                        // will always be aligned
                        let val = psx.bus_read_u32(addr).unwrap();
                        let instr = Instruction::from_u32(val, addr);
                        println!(
                            "0x{:08X}: {}{}",
                            addr,
                            if previous_instr.is_branch() { "_" } else { "" },
                            instr
                        );
                        previous_instr = instr;
                    }
                } else {
                    println!("Error reading u32 {:08X}: {:?}", addr - 4, previous_instr_d);
                }
            }
            "spu" => {
                psx.print_spu_state();
            }
            "hook_add" => {
                if let Some(arg) = arg {
                    for split in arg.split(';') {
                        self.breakpoint_hooks.push(split.to_string());
                        println!("Hook added: {}", split);
                    }
                } else {
                    println!("Usage: hook_add <command>");
                }
            }
            "hook_clear" => {
                self.breakpoint_hooks.clear();
                println!("Hooks cleared");
            }
            "hook_list" => {
                for hook in &self.breakpoint_hooks {
                    println!("{}", hook);
                }
            }
            "hook_setting" => {
                if let Some(arg) = arg {
                    for split in arg.split(',') {
                        let one_setting = split.trim().split_once('=');
                        let (break_type, new_value) = match one_setting {
                            Some((break_type, new_value)) => {
                                let new_value = new_value.trim();
                                match new_value.to_ascii_lowercase().as_str() {
                                    "true" | "t" => (break_type.trim(), true),
                                    "false" | "f" => (break_type.trim(), false),
                                    _ => {
                                        println!("Invalid value set: {}", new_value);
                                        continue;
                                    }
                                }
                            }
                            None => (split.trim(), true),
                        };

                        match break_type {
                            "step" => self.run_hook_settings.step = new_value,
                            "step_over" => self.run_hook_settings.step_over = new_value,
                            "step_out" => self.run_hook_settings.step_out = new_value,
                            "instruction_breakpoint" => {
                                self.run_hook_settings.instruction_breakpoint = new_value
                            }
                            "read_breakpoint" => self.run_hook_settings.read_breakpoint = new_value,
                            "write_breakpoint" => {
                                self.run_hook_settings.write_breakpoint = new_value
                            }
                            _ => {
                                println!("Invalid breakpoint type: {}", break_type);
                                continue;
                            }
                        }
                    }
                }

                println!("Hooks will be executed on the following breakpoints:");
                println!("  step: {}", self.run_hook_settings.step);
                println!("  step_over: {}", self.run_hook_settings.step_over);
                println!("  step_out: {}", self.run_hook_settings.step_out);
                println!(
                    "  instruction_breakpoint: {}",
                    self.run_hook_settings.instruction_breakpoint
                );
                println!(
                    "  read_breakpoint: {}",
                    self.run_hook_settings.read_breakpoint
                );
                println!(
                    "  write_breakpoint: {}",
                    self.run_hook_settings.write_breakpoint
                );
            }
            "" => {}
            _ => println!("Unknown command: {}", cmd),
        }
    }

    fn run_hooks(&mut self, psx: &mut Psx) {
        let hooks = std::mem::take(&mut self.breakpoint_hooks);
        for hook in &hooks {
            self.handle_command(psx, hook);
        }
        self.breakpoint_hooks = hooks;
    }

    pub fn handle_cpu_state(&mut self, psx: &mut Psx, cpu_state: CpuState) {
        match cpu_state {
            CpuState::Normal => {}
            CpuState::InstructionBreakpoint(addr) => {
                println!("Instruction breakpoint at {:#x}", addr);
                self.set_enabled(true);
                if self.run_hook_settings.instruction_breakpoint {
                    self.run_hooks(psx);
                }
            }
            CpuState::WriteBreakpoint { addr, bits } => {
                let hw_reg_name = HW_REGISTERS
                    .entries()
                    .find(|(_, &v)| v == addr)
                    .map(|(k, _)| format!(" [@{}]", k))
                    .unwrap_or("".to_string());

                println!(
                    "Write breakpoint at {:08x}{} with bits {:02} at {:08x}",
                    addr,
                    hw_reg_name,
                    bits,
                    psx.cpu().registers().read(RegisterType::Pc)
                );

                self.set_enabled(true);
                if self.run_hook_settings.write_breakpoint {
                    self.run_hooks(psx);
                }
            }
            CpuState::ReadBreakpoint { addr, bits } => {
                let hw_reg_name = HW_REGISTERS
                    .entries()
                    .find(|(_, &v)| v == addr)
                    .map(|(k, _)| format!(" [@{}]", k))
                    .unwrap_or("".to_string());

                println!(
                    "Read breakpoint at {:08x}{} with bits {:02} at {:08x}",
                    addr,
                    hw_reg_name,
                    bits,
                    psx.cpu().registers().read(RegisterType::Pc)
                );

                self.set_enabled(true);
                if self.run_hook_settings.read_breakpoint {
                    self.run_hooks(psx);
                }
            }
            CpuState::Step => {
                self.set_enabled(true);
                if self.run_hook_settings.step {
                    self.run_hooks(psx);
                }
            }
            CpuState::StepOver => {
                self.set_enabled(true);
                if self.run_hook_settings.step_over {
                    self.run_hooks(psx);
                }
            }
            CpuState::StepOut => {
                self.set_enabled(true);
                if self.run_hook_settings.step_out {
                    self.run_hooks(psx);
                }
            }
        }
    }
}
