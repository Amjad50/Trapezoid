<!-- <p align="center">
  <a href="https://github.com/Amjad50/trapezoid"><img alt="trapezoid" src="images/logo.svg" width="60%"></a>
  <p align="center">PSX emulator in <em>Rust</em></p>
</p> -->
# Trapezoid

[![Build status](https://github.com/Amjad50/trapezoid/workflows/Rust/badge.svg)](https://actions-badge.atrox.dev/Amjad50/trapezoid/goto)
[![dependency status](https://deps.rs/repo/github/Amjad50/trapezoid/status.svg)](https://deps.rs/repo/github/Amjad50/trapezoid)
[![license](https://img.shields.io/github/license/Amjad50/trapezoid)](./LICENSE)
[![Crates.io trapezoid](https://img.shields.io/crates/v/trapezoid)](https://crates.io/crates/trapezoid)

**trapezoid** is a [PSX/PS1](https://en.wikipedia.org/wiki/PlayStation_(console)) emulator built from scratch using [Rust].

This is a personal project for fun and to experience emulating hardware and connecting them together.

## Showcase

<img src="assets/psx.png" alt="demo" width="600"/>

## Building and installation

### Installing
You can install `trapezoid` from [`crates.io`](https://crates.io/crates/trapezoid) using `cargo`:
```
cargo install trapezoid
```

### Building
If you want to experience the latest development version, you can build `trapezoid` yourself.
```
cargo build --release
```
> The emulator will be slow without optimization, that's why we have `opt-level = 2` in `debug` profile.

## Emulator core
The emulator core is implemented as a library in [`trapezoid-core`], this library is the emulator core, and contain
all the components. You can easily take the core and build a frontend around it, or use it as a server.

Check the [`trapezoid-core`] for more info and documentation.

## Frontend

### Controls
The Frontend implementations has its own controls mapping, this can be configured
if you decide to use [`trapezoid-core`] directly

#### Keyboard
| keyboard  | PSX controller |
| --------- | -------------- |
| Enter     | Start          |
| Backspace | Select         |
| Num1      | L1             |
| Num2      | L2             |
| Num3      | L3             |
| Num0      | R1             |
| Num9      | R2             |
| Num8      | R3             |
| W         | Up             |
| S         | Down           |
| D         | Right          |
| A         | Left           |
| I         | Triangle       |
| K         | X              |
| L         | Circle         |
| J         | Square         |

### Debugging
`trapezoid` has a built-in powerfull debugger to help debug games and access to data.

This is a CLI based debugger, it can be activated by pressing `/ (forward slash)` key, it will pause the emulation and activates
the debugger.

You will get a prompt:
```text
CPU>
```
The debugger uses `rustyline` and has auto completion

#### Debugger addressing and variables
Anywhere the term `<addr>` is used, it can be a hex address, or a variable name.

There are two variable types:
- start with `$` are registers, for example `$t0` is the register `t0`, etc...
- start with `@` are special hardware registers, like `@TIMER0_TARGET` which is the timer 0 target register.

You can know these registers using the tab completion. Just start typing `$` or `@` and press tab.

#### Debugger commands

##### `h`
Prints the help message
```txt
CPU> h
h - help
r - print registers
c - continue
s - step
so - step-over
su - step-out
tt - enable trace
tf - disbale trace
stack [0xn] - print stack [n entries in hex]
bt/[limit] - print backtrace [top `limit` entries]
b <addr> - set breakpoint
rb <addr> - remove breakpoint
bw <addr> - set write breakpoint
rbw <addr> - remove write breakpoint
br <addr> - set read breakpoint
rbr <addr> - remove read breakpoint
lb - list breakpoints
m[32/16/8] <addr> - print content of memory (default u32)
p <addr>/<$reg> - print address or register value
i/[n] [addr] - disassemble instructions
hook_add <cmd[;cmd]> - add hook/s commands
hook_clear - clear all hooks
hook_list - list all hooks
hook_setting [<break_type>[=true/false]] - change when the hooks are executed
```

##### `r`
Prints the registers (example from a random game in a random point)
```txt
CPU> r
Registers:
pc: 8004A648    at: 80060000
hi: 00000000    lo: 009941F4
v0: 00003178    s0: 54042275
v1: FFFFFFFF    s1: 0000015B
a0: 00003179    s2: 0000008F
a1: 00008000    s3: 00000000
a2: 00000000    s4: 00000002
a3: 00000000    s5: 00000000
t0: 39937A40    s6: 00000000
t1: 00000000    s7: 00000000
t2: 00000000    t8: 00000000
t3: F9A700FE    t9: 801FFEE0
t4: 0000F159    k0: 8004A600
t5: 801A1D9C    k1: 00006418
t6: 00000001    gp: 8005F17C
t7: 00000003    sp: 801FFE78
fp: 801FFFF8    ra: 8004A540
```

##### `c`
Continue the emulation

##### `s`
Executes one instruction and then stops

##### `so`
Executes one instruction and then stops, if the instruction is a function call, it will execute the function and stop at the next instruction
after the call.

For example, if the code was like this
```asm
0x1000: jal 0x8004A648
0x1004: _nop            ; delay slot
0x1008: nop
```
and the PC was at `0x1000`, then `so` will execute `jal` and stop at `0x1008`

##### `su`
Will continue the emulation until the current function returns.

It will stop on the instruction after the function call.

##### `tt`
Enable trace, this will print the executed instructions, this is very heavy as it prints all instruction and will reduce the emulation speed.

Example output:
```txt
CPU> tt
Instruction trace: true
CPU> c
80000080: lui k0, 0x0000
80000084: addiu k0, k0, 0x0C80
80000088: jr k0
8000008C: _nop
00000C80: nop
00000C84: nop
00000C88: addiu k0, zero, 0x0100
00000C8C: lw k0, 0x0008(k0)
00000C90: nop
00000C94: lw k0, 0x0000(k0)
00000C98: nop
...
```

##### `tf`
Disable trace

##### `stack`
Print the stack content, you can specify the number of entries to print, default is 10
```txt
CPU> stack
Stack: SP=0x801FFC90
    8001273C
    8001273C
    00000002
    00000000
    00000000
    800C82AC
    00000012
    00000001
    00000000
    800143AC
```

##### `bt`
Print the backtrace, you can specify the number of entries to print, default, whole backtrace

For example, here we are in `59` level of the backtrace, but we only print the top 10 entries
```txt
CPU> bt/10
#59:      80012E24
#58:      000019B8
#57:      00000E28
#56:      8004AAB0
#55:      8004A888
#54:      8004AAB0
#53:      8004A888
#52:      000019B8
#51:      00000E28
#50:      000019B8
```

The addresses here are the return addresses, for example, looking at the first one `80012E24`, lets print the 2 instructions before it.

We will we have the call instruction, the delay slot, and the return address is what's in the backtrace.
```txt
CPU> i 80012E1C
0x80012E1C: jal 0x0004AF1 => 0x80012BC4
0x80012E20: _addiu a0, zero, 0xFFFF
0x80012E24: lui v0, 0x8006
```
This means that right now we are inside the function `0x80012BC4`

##### `b`
Set a breakpoint on address, the address is in hex, the `0x` prefix is optional
This will trigger when the address is executed
```txt
CPU> b 80012E24
Breakpoint added: 0x80012E24
```

##### `rb`
Remove a breakpoint
```txt
CPU> rb 80012E24
Breakpoint removed: 0x80012E24
```

##### `bw`
Set a write breakpoint on address, the address is in hex, the `0x` prefix is optional
This will trigger when the address is written to
```txt
CPU> bw 80012E24
Write breakpoint added: 0x80012E24
```

##### `rbw`
Remove a write breakpoint
```txt
CPU> rbw 80012E24
Write breakpoint removed: 0x80012E24
```

##### `br`
Set a read breakpoint on address, the address is in hex, the `0x` prefix is optional
This will trigger when the address is read from (also execute, since we are reading from this address)
```txt
CPU> br 80012E24
Read breakpoint added: 0x80012E24
```

##### `rbr`
Remove a read breakpoint
```txt
CPU> rbr 80012E24
Read breakpoint removed: 0x80012E24
```

##### `lb`
List all breakpoints
```txt
CPU> lb
Breakpoint: 0x80012E24
Write Breakpoint: 0x80012E24
Read Breakpoint: 0x80012E24
```

##### `m`
Print the memory content, you can specify the size of the read, and the number of times to read, default is 1 u32

```txt
CPU> m 80012E24
0x80012E24: 0x3C028006
CPU> m32 80012E24
0x80012E24: 0x3C028006
CPU> m32/4 80012E24
0x80012E24: 0x3C028006
0x80012E28: 0x8C427FB0
0x80012E2C: 0x00000000
0x80012E30: 0x1440FFED
CPU> m8/4 80012E24
0x80012E24: 0x06
0x80012E25: 0x80
0x80012E26: 0x02
0x80012E27: 0x3C
CPU> m16/4 80012E24
0x80012E24: 0x8006
0x80012E26: 0x3C02
0x80012E28: 0x7FB0
0x80012E2A: 0x8C42

CPU> m @GPU_STAT        ; reading gpu status register easily
0x1F801814: 0x5404220A
```

##### `p`
Print the value of a register or memory address

This is only useful for cpu registers, at least for now, there is no expression evaluation
```txt
CPU> p $t0
0x00005688
CPU> p @GPU_STAT
0x1F801814
CPU> p 12345678
0x12345678
```

##### `i`
Disassemble instructions, you can specify the number of instructions to disassemble, default is 1 at the current location of `PC`

```txt
CPU> i
0x80000084: addiu k0, k0, 0x0C80
CPU> i/10
0x80000084: addiu k0, k0, 0x0C80
0x80000088: jr k0
0x8000008C: _nop
0x80000090: nop
0x80000094: nop
0x80000098: nop
0x8000009C: nop
0x800000A0: lui t0, 0x0000
0x800000A4: addiu t0, t0, 0x05C4
0x800000A8: jr t0
CPU> i 800000A0
0x800000A0: lui t0, 0x0000
```

#### Hooks

The debugger allows to create `hooks`, these are commands, any of the above commands which will execute on certain events.
The events can be configured using `hook_setting` command.
```txt
CPU> hook_setting
Hooks will be executed on the following breakpoints:
  step: false
  step_over: false
  step_out: false
  instruction_breakpoint: false
  read_breakpoint: false
  write_breakpoint: false
```

By default, hooks aren't bound to any event.

But can be set using `hook_setting` to modify when to execute them.
```txt
CPU> hook_setting step,instruction_breakpoint=true,step_out=false
Hooks will be executed on the following breakpoints:
  step: true
  step_over: false
  step_out: false
  instruction_breakpoint: true
  read_breakpoint: false
  write_breakpoint: false
```
This will enable hooks on `step` and `instruction_breakpoint` events, and disable them on `step_out` event, and leave the rest as is.

##### `hook_add`
We can add hooks by `hook_add`, which will be executed when the event is triggered.
```txt
CPU> hook_add r;i/20
Hook added: r
Hook added: i/20
```
This adds two commands to execute on an event, `r` and `i/20`, `r` will print the registers, and `i/20` will disassemble 20 instructions from `PC`.

##### `hook_clear`
Clear all hooks

##### `hook_list`
List all hooks

```txt
CPU> hook_list
r
i/20
```

#### VRAM

We can view the raw vram state, which you can think of as an image of 1024x512 pixels

This can be triggerd with the keyboard button `v`.

![vram](assets/psx_vram.png)


### License
This project is under [MIT](./LICENSE) license.

NES is a product and/or trademark of Nintendo Co., Ltd. Nintendo Co., Ltd. and is not affiliated in any way with Plastic or its author

### References
Most of the documentation for PSX components can be found in the [consoledev website](https://psx-spx.consoledev.net/)

[Rust]: https://www.rust-lang.org/
[`trapezoid-core`]: ./trapezoid-core/README.md