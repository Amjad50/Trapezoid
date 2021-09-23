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

pub use controller_mem_card::DigitalControllerKey;

pub struct Psx {
    bus: CpuBus,
    cpu: Cpu,
}

impl Psx {
    // TODO: produce a valid `Error` struct
    pub fn new<BiosPath: AsRef<Path>, DiskPath: AsRef<Path>, F: glium::backend::Facade>(
        bios_file_path: BiosPath,
        disk_file: Option<DiskPath>,
        gl_facade: &F,
    ) -> Result<Self, ()> {
        let bios = Bios::from_file(bios_file_path)?;

        Ok(Self {
            cpu: Cpu::new(),
            bus: CpuBus::new(bios, disk_file, GlContext::new(gl_facade)),
        })
    }

    /// return `true` on the beginning of VBLANK
    pub fn clock(&mut self) -> bool {
        let in_vblank_old = self.bus.gpu().in_vblank();
        self.cpu.execute_next(&mut self.bus);

        self.bus.gpu().in_vblank() && !in_vblank_old
    }

    pub fn change_controller_key_state(&mut self, key: DigitalControllerKey, pressed: bool) {
        self.bus
            .controller_mem_card_mut()
            .change_controller_key_state(key, pressed);
    }

    pub fn blit_to_front<S: glium::Surface>(&self, s: &S, full_vram: bool) {
        self.bus.gpu().blit_to_front(s, full_vram);
    }
}
