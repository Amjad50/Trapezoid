mod cdrom;
mod controller_mem_card;
mod coprocessor;
mod cpu;
mod gpu;
mod memory;
mod spu;
mod timers;

use std::path::Path;

use cpu::Cpu;
use gpu::GlContext;
use memory::{Bios, CpuBus};

pub struct Psx {
    bus: CpuBus,
    cpu: Cpu,
}

impl Psx {
    // TODO: produce a valid `Error` struct
    pub fn new<P: AsRef<Path>, F: glium::backend::Facade>(
        bios_file_path: P,
        gl_facade: &F,
    ) -> Result<Self, ()> {
        let bios = Bios::from_file(bios_file_path)?;

        Ok(Self {
            cpu: Cpu::new(),
            bus: CpuBus::new(bios, GlContext::new(gl_facade)),
        })
    }

    /// return `true` on the beginning of VBLANK
    pub fn clock(&mut self) -> bool {
        let in_vblank_old = self.bus.gpu().in_vblank();
        self.cpu.execute_next(&mut self.bus);

        self.bus.gpu().in_vblank() && !in_vblank_old
    }

    pub fn blit_to_front<S: glium::Surface>(&self, s: &S) {
        self.bus.gpu().blit_to_front(s);
    }
}
