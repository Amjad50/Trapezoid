mod cpu;
mod memory;

use std::path::Path;

use cpu::Cpu;
use memory::{Bios, CpuBus};

pub struct Psx {
    bus: CpuBus,
    cpu: Cpu,
}

impl Psx {
    // TODO: produce a valid `Error` struct
    pub fn new<P: AsRef<Path>>(bios_file_path: P) -> Result<Self, ()> {
        let bios = Bios::from_file(bios_file_path)?;

        Ok(Self {
            cpu: Cpu::new(),
            bus: CpuBus::new(bios),
        })
    }

    pub fn clock(&mut self) {
        self.cpu.execute_next(&mut self.bus);
    }
}
