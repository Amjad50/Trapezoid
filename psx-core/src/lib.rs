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

use std::{
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc::{channel, Receiver, Sender},
        Arc,
    },
    time::Instant,
};

use cpu::Cpu;
use gpu::GpuRenderer;
use memory::{Bios, CpuBus};

pub use controller_mem_card::DigitalControllerKey;
use vulkano::{
    device::{Device, Queue},
    image::ImageAccess,
    sync::GpuFuture,
};

const MAX_CPU_CYCLES_TO_CLOCK: u32 = 1000;

enum PsxCommand {
    ChangeKeyState(DigitalControllerKey, bool),
}

// quick and dirty way to have atomic f64
pub struct AtomicF64 {
    storage: AtomicU64,
}
impl AtomicF64 {
    pub fn new(value: f64) -> Self {
        let as_u64 = value.to_bits();
        Self {
            storage: AtomicU64::new(as_u64),
        }
    }
    pub fn store(&self, value: f64, ordering: Ordering) {
        let as_u64 = value.to_bits();
        self.storage.store(as_u64, ordering)
    }
    pub fn load(&self, ordering: Ordering) -> f64 {
        let as_u64 = self.storage.load(ordering);
        f64::from_bits(as_u64)
    }
}

struct PsxBackend {
    bus: CpuBus,
    cpu: Cpu,
    /// Stores the excess CPU cycles for later execution.
    ///
    /// Sometimes, when running the DMA (mostly CD-ROM) it can generate
    /// a lot of CPU cycles, clocking the components with this many CPU cycles
    /// will crash the emulator, so we split clocking across multiple `clock` calls.
    excess_cpu_cycles: u32,

    cmd_rx: Receiver<PsxCommand>,

    instant: Instant,
    fps: Arc<AtomicF64>,
}

impl PsxBackend {
    pub fn run(&mut self) {
        loop {
            let in_vblank = self.bus.gpu().in_vblank();
            for _ in 0..100 {
                self.clock();
            }
            if !in_vblank && self.bus.gpu().in_vblank() {
                self.fps.store(
                    1.0 / self.instant.elapsed().as_secs_f64(),
                    Ordering::Relaxed,
                );
                self.instant = Instant::now();
            }

            match self.cmd_rx.try_recv() {
                Ok(PsxCommand::ChangeKeyState(key, pressed)) => {
                    self.change_controller_key_state(key, pressed);
                }
                Err(_) => {}
            }
        }
    }

    fn clock(&mut self) {
        if self.excess_cpu_cycles == 0 {
            // this number doesn't mean anything
            // TODO: research on when to stop the CPU (maybe fixed number? block of code? other?)
            let cpu_cycles = self.cpu.clock(&mut self.bus, 32);

            // the DMA is running of the CPU
            self.excess_cpu_cycles = cpu_cycles + self.bus.clock_dma();
        }

        let cpu_cycles_to_run = self.excess_cpu_cycles.min(MAX_CPU_CYCLES_TO_CLOCK);
        self.excess_cpu_cycles -= cpu_cycles_to_run;
        self.bus.clock_components(cpu_cycles_to_run);
    }

    fn change_controller_key_state(&mut self, key: DigitalControllerKey, pressed: bool) {
        self.bus
            .controller_mem_card_mut()
            .change_controller_key_state(key, pressed);
    }
}

pub struct Psx {
    cmd_tx: Sender<PsxCommand>,
    gpu_renderer: GpuRenderer,
    fps: Arc<AtomicF64>,
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

        let (cmd_tx, cmd_rx) = channel();

        let fps = Arc::new(AtomicF64::new(0.0));

        let backend = PsxBackend {
            cpu: Cpu::new(),
            bus: CpuBus::new(bios, disk_file, device, queue),
            excess_cpu_cycles: 0,
            cmd_rx,
            instant: Instant::now(),
            fps: fps.clone(),
        };

        let gpu_renderer = backend.bus.gpu().create_renderer();

        std::thread::spawn(move || {
            let mut backend = backend;
            backend.run();
        });

        Ok(Self {
            cmd_tx,
            gpu_renderer,
            fps,
        })
    }

    pub fn change_controller_key_state(&mut self, key: DigitalControllerKey, pressed: bool) {
        self.cmd_tx
            .send(PsxCommand::ChangeKeyState(key, pressed))
            .unwrap();
    }

    pub fn blit_to_front<D, IF>(&mut self, dest_image: Arc<D>, full_vram: bool, in_future: IF)
    where
        D: ImageAccess + 'static,
        IF: GpuFuture + Send + 'static,
    {
        self.gpu_renderer
            .sync_gpu_and_blit_to_front(dest_image, full_vram, in_future);
    }

    pub fn emulation_fps(&self) -> f64 {
        self.fps.load(Ordering::Relaxed)
    }
}
