mod cdrom;
mod controller_mem_card;
mod coprocessor;
mod cpu;
mod gpu;
mod mdec;
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

const MAX_CPU_CYCLES_TO_CLOCK: u32 = 1000;

#[derive(Debug, Clone, Copy)]
pub struct PsxConfig {
    pub stdout_debug: bool,
}

pub struct Psx {
    bus: CpuBus,
    cpu: Cpu,
    /// Stores the excess CPU cycles for later execution.
    ///
    /// Sometimes, when running the DMA (mostly CD-ROM) it can generate
    /// a lot of CPU cycles, clocking the components with this many CPU cycles
    /// will crash the emulator, so we split clocking across multiple `clock` calls.
    excess_cpu_cycles: u32,
    cpu_frame_cycles: u32,
}

impl Psx {
    // TODO: produce a valid `Error` struct
    pub fn new<BiosPath: AsRef<Path>, DiskPath: AsRef<Path>>(
        bios_file_path: BiosPath,
        disk_file: Option<DiskPath>,
        config: PsxConfig,
        device: Arc<Device>,
        queue: Arc<Queue>,
    ) -> Result<Self, ()> {
        let bios = Bios::from_file(bios_file_path)?;

        Ok(Self {
            cpu: Cpu::new(),
            bus: CpuBus::new(bios, disk_file, config, device, queue),
            excess_cpu_cycles: 0,
            cpu_frame_cycles: 0,
        })
    }

    pub fn clock_frame(&mut self) {
        // sync the CPU clocks to the SPU so that the audio would be clearer.
        const CYCLES_PER_FRAME: u32 = 564480;

        while self.cpu_frame_cycles < CYCLES_PER_FRAME {
            if self.excess_cpu_cycles == 0 {
                // this number doesn't mean anything
                // TODO: research on when to stop the CPU (maybe fixed number? block of code? other?)
                let cpu_cycles = self.cpu.clock(&mut self.bus, 32);
                if cpu_cycles == 0 {
                    return;
                }
                // the DMA is running of the CPU
                self.excess_cpu_cycles = cpu_cycles + self.bus.clock_dma();
                self.cpu_frame_cycles += self.excess_cpu_cycles;
            }

            let cpu_cycles_to_run = self.excess_cpu_cycles.min(MAX_CPU_CYCLES_TO_CLOCK);
            self.excess_cpu_cycles -= cpu_cycles_to_run;
            self.bus.clock_components(cpu_cycles_to_run);
        }
        self.cpu_frame_cycles -= CYCLES_PER_FRAME;
    }

    pub fn change_controller_key_state(&mut self, key: DigitalControllerKey, pressed: bool) {
        self.bus
            .controller_mem_card_mut()
            .change_controller_key_state(key, pressed);
    }

    pub fn blit_to_front<D>(
        &mut self,
        dest_image: Arc<D>,
        full_vram: bool,
        in_future: Box<dyn GpuFuture>,
    ) -> Box<dyn GpuFuture>
    where
        D: ImageAccess + 'static,
    {
        self.bus
            .gpu_mut()
            .sync_gpu_and_blit_to_front(dest_image, full_vram, in_future)
    }

    pub fn take_audio_buffer(&mut self) -> Vec<i16> {
        self.bus.spu_mut().take_audio_buffer()
    }

    pub fn pause_cpu(&mut self) {
        self.cpu.set_pause(true);
        #[cfg(feature = "debugger")]
        self.cpu.print_cpu_registers();
    }
}
