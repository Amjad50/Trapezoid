//! This is an example of how to use the `Cpu` for standalone emulation.
//!
//! Here, we have a custom memory Bus that implements our reads and writes.
//!
//! We have made some specific "hardware registers" to interact with the outside world.
//!
//! We have set `0x0` to be the exit register, and `0x4` to be the write character register.
//!
//! Beside that, everything is in the memory at `0xBFC00000..=0xBFC0FFFF`. because that's where the
//! PC start and that's the reset vector location. We can jump anywhere from there.
//!
//! Though, for now, the CPU won't crash or interrupt if it tries to read unsupported locations.
//! It will just be logged, so that it doesn't affect the PSX operations
//! (make sure you have logger setup if you want to get notified).
//!
//! The simple code is compiled below, view the commented code for more details.
//!

use trapezoid_core::cpu::{BusLine, Cpu, CpuBusProvider};

struct Bus {
    kseg1: [u8; 0x10000],
    is_done: bool,
}

impl BusLine for Bus {
    // just maps the kseg1 memory to the CPU
    fn read_u32(&mut self, addr: u32) -> Result<u32, String> {
        match addr {
            0xBFC00000..=0xBFC0FFFF => {
                let offset = addr - 0xBFC00000;
                let word = u32::from_le_bytes([
                    self.kseg1[offset as usize],
                    self.kseg1[(offset + 1) as usize],
                    self.kseg1[(offset + 2) as usize],
                    self.kseg1[(offset + 3) as usize],
                ]);
                Ok(word)
            }
            _ => Err(format!("Unimplemented read at address {:08X}", addr)),
        }
    }

    // just maps the kseg1 memory to the CPU, needed to read the string data
    fn read_u8(&mut self, addr: u32) -> Result<u8, String> {
        match addr {
            0xBFC00000..=0xBFC0FFFF => {
                let offset = addr - 0xBFC00000;
                Ok(self.kseg1[offset as usize])
            }
            _ => Err(format!("Unimplemented read at address {:08X}", addr)),
        }
    }

    // Ability to write to the stack location, here we allow whole kseg1, but can be limited as
    // needed
    fn write_u32(&mut self, addr: u32, data: u32) -> Result<(), String> {
        match addr {
            0xBFC00000..=0xBFC0FFFF => {
                let offset = addr - 0xBFC00000;
                self.kseg1[offset as usize] = data as u8;
                self.kseg1[(offset + 1) as usize] = (data >> 8) as u8;
                self.kseg1[(offset + 2) as usize] = (data >> 16) as u8;
                self.kseg1[(offset + 3) as usize] = (data >> 24) as u8;
                Ok(())
            }
            _ => Err(format!("Unimplemented write at address {:08X}", addr)),
        }
    }

    // support our custom registers
    fn write_u8(&mut self, addr: u32, data: u8) -> Result<(), String> {
        match addr {
            // exit
            0x0 => {
                println!("Write to address 0x0: {:08X}, exiting", data);
                self.is_done = data != 0x0;
                Ok(())
            }
            // write a character
            0x4 => {
                print!("{}", data as char);
                Ok(())
            }
            _ => Err(format!("Unimplemented write at address {:08X}", addr)),
        }
    }
}

// Not used for now, we don't have interrupts handling from the hardware
// If we do, when `pending_interrupts` returns true, the CPU will jump to the interrupt vector with
// cause `Interrupt` (not available for public API), but the interrupt vector is
// `0xBFC00180` or `0x80000080`.
impl CpuBusProvider for Bus {
    fn pending_interrupts(&self) -> bool {
        false
    }

    fn should_run_dma(&self) -> bool {
        false
    }
}

/// This is a compiled code of the following assembly code:
/// ```asm
/// ; jump to start, we have it at the end for forward declaration of `print_string`
/// j start
/// nop
///
/// print_string:
///         ; epilogue
///         addiu   $sp, $sp, -8
///         sw      $ra, 4($sp)
///         sw      $fp, 0($sp)
///         move    $fp, $sp
///
///         ; print loop ($a0 is the string pointer, $a1 is the string length)
/// loop:
///         beqz    $a1, outside
///         nop
///         lbu     $v0, 0($a0)
///         addiu   $a0, $a0, 1
///         sub     $a1, $a1, 1
///         sb      $v0, 4($zero)
///         j       loop
///         nop
/// outside:
///         ; epilogue
///         move    $sp, $fp
///         lw      $fp, 0($sp)
///         lw      $ra, 4($sp)
///         addiu   $sp, $sp, 8
///         jr      $ra
///         nop
///
/// start:
///         ; setup the stack at 0xBFC0E000
///         lui     $sp, 0xBFC0
///         ori     $sp, $sp, 0xE000
///
///         ; setup the string pointer and length (0xBFC0D000, 14)
///         lui     $a0, 0xBFC0
///         ori     $a0, $a0, 0xD000
///         addiu   $a1, $zer0, 14
///
///         ; call print_string
///         jal     print_string
///         nop
///
///         ; exit via register
///         addiu   $v0, $zero, 1
///         sb      $v0, 0($zero)
///
///         ; nothing should happen after this
/// ```
const CODE: [u8; 136] = [
    0x17, 0x00, 0xf0, 0x0b, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0xf8, 0xff, 0xbd, 0x27,
    0x04, 0x00, 0xbf, 0xaf, 0x00, 0x00, 0xbe, 0xaf, 0x25, 0xf0, 0xa0, 0x03, 0x08, 0x00, 0xa0, 0x10,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x82, 0x90, 0x01, 0x00, 0x84, 0x24,
    0xff, 0xff, 0xa5, 0x20, 0x04, 0x00, 0x02, 0xa0, 0x07, 0x00, 0xf0, 0x0b, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x25, 0xe8, 0xc0, 0x03, 0x00, 0x00, 0xbe, 0x8f, 0x04, 0x00, 0xbf, 0x8f,
    0x08, 0x00, 0xbd, 0x27, 0x08, 0x00, 0xe0, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xc0, 0xbf, 0x1d, 0x3c, 0x00, 0xe0, 0xbd, 0x37, 0xc0, 0xbf, 0x04, 0x3c, 0x00, 0xd0, 0x84, 0x34,
    0x0e, 0x00, 0xa5, 0x24, 0x03, 0x00, 0xf0, 0x0f, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x01, 0x00, 0x02, 0x24, 0x00, 0x00, 0x02, 0xa0,
];

const STRING_TO_PRINT: &[u8; 15] = b"Hello, world!\n\0";

fn main() {
    // collect logs
    env_logger::init();

    let mut cpu = Cpu::new();
    let mut kseg1 = [0; 0x10000];
    // copy the code
    kseg1[0..CODE.len()].copy_from_slice(&CODE);
    // copy the string to 0xBFC0D000
    kseg1[0xD000..0xD000 + STRING_TO_PRINT.len()].copy_from_slice(STRING_TO_PRINT);

    let mut bus = Bus {
        kseg1,
        is_done: false,
    };

    let mut total_cycles = 0;
    while !bus.is_done {
        let (n_cycles, _cpu_state) = cpu.clock(&mut bus, 1);
        total_cycles += n_cycles;
    }

    println!("Total cycles: {}", total_cycles);
}
