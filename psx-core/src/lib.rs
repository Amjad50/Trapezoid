mod cdrom;
mod controller_mem_card;
mod coprocessor;
mod cpu;
mod gpu;
mod memory;
mod spu;
mod timers;

#[cfg(test)]
mod tests;

use std::{path::Path, sync::Arc};

use cpu::Cpu;
use memory::{Bios, CpuBus};

pub use controller_mem_card::DigitalControllerKey;
use vulkano::{
    device::{Device, Queue},
    image::ImageAccess,
    sync::GpuFuture,
};

pub struct Psx {
    bus: CpuBus,
    cpu: Cpu,
}

impl Psx {
    // TODO: produce a valid `Error` struct
    pub fn new<BiosPath: AsRef<Path>, DiskPath: AsRef<Path>>(
        bios_file_path: BiosPath,
        disk_file: Option<DiskPath>,
        device: Arc<Device>,
        queue: Arc<Queue>,
    ) -> Result<Self, ()> {
        let bios = Bios::from_file(bios_file_path)?;

        Ok(Self {
            cpu: Cpu::new(),
            bus: CpuBus::new(bios, disk_file, device, queue),
        })
    }

    /// return `true` on the beginning of VBLANK
    pub fn clock(&mut self) -> bool {
        let in_vblank_old = self.bus.gpu().in_vblank();

        // this number doesn't mean anything
        // TODO: research on when to stop the CPU (maybe fixed number? block of code? other?)
        for _ in 0..32 {
            self.cpu.execute_next(&mut self.bus);
        }
        self.bus.clock_components(self.cpu.take_elapsed_cycles());

        self.bus.gpu().in_vblank() && !in_vblank_old
    }

    pub fn change_controller_key_state(&mut self, key: DigitalControllerKey, pressed: bool) {
        self.bus
            .controller_mem_card_mut()
            .change_controller_key_state(key, pressed);
    }

    pub fn blit_to_front<D, IF>(&mut self, dest_image: Arc<D>, full_vram: bool, in_future: IF)
    where
        D: ImageAccess + 'static,
        IF: GpuFuture,
    {
        self.bus
            .gpu_mut()
            .blit_to_front(dest_image, full_vram, in_future);
    }
}
