#![cfg_attr(docsrs, feature(doc_cfg))]

mod cdrom;
mod controller_mem_card;
mod coprocessor;
pub mod cpu;
pub mod gpu;
mod mdec;
mod memory;
mod spu;
mod timers;

#[cfg(test)]
mod tests;

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use cpu::RegisterType;
use memory::{Bios, BusLine, CpuBus, Result};

pub use controller_mem_card::DigitalControllerKey;

use crate::gpu::{Device, GpuFuture, Image, Queue};

const MAX_CPU_CYCLES_TO_CLOCK: u32 = 2000;

#[derive(Debug)]
pub enum PsxError {
    CouldNotLoadBios,
    CouldNotLoadDisk(String),
    DiskTypeNotSupported,
}

impl std::error::Error for PsxError {}
impl std::fmt::Display for PsxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PsxError::CouldNotLoadBios => write!(f, "Could not load BIOS"),
            PsxError::CouldNotLoadDisk(s) => write!(f, "Could not load disk: {}", s),
            PsxError::DiskTypeNotSupported => write!(f, "Disk type not supported"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PsxConfig {
    pub stdout_debug: bool,
    pub fast_boot: bool,
}

pub struct Psx {
    bus: CpuBus,
    exe_file: Option<PathBuf>,
    // used to control when to execute fastboot
    disk_available: bool,
    config: PsxConfig,
    cpu: cpu::Cpu,
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
    ) -> Result<Self, PsxError> {
        let bios = Bios::from_file(bios_file_path)?;

        // save the exe file if there is any
        // The PSX itself is only responsible for loading normal cue files
        let (exe_file, disk_file) = if let Some(disk_file) = disk_file {
            let path = disk_file.as_ref().to_owned();
            // if this is an exe file
            match path
                .extension()
                .unwrap()
                .to_str()
                .unwrap()
                .to_ascii_lowercase()
                .as_str()
            {
                "exe" => (Some(path), None),
                "cue" => (None, Some(path)),
                _ => {
                    return Err(PsxError::DiskTypeNotSupported);
                }
            }
        } else {
            // only fast_boot if there is anything to run
            (None, None)
        };

        Ok(Self {
            cpu: cpu::Cpu::new(),
            disk_available: disk_file.is_some(),
            bus: CpuBus::new(bios, disk_file, config, device, queue)?,
            exe_file,
            config,
            excess_cpu_cycles: 0,
            cpu_frame_cycles: 0,
        })
    }

    pub fn reset(&mut self) {
        self.cpu.reset();
        self.bus.reset();
    }

    #[inline(always)]
    fn common_clock(&mut self) -> (u32, cpu::CpuState) {
        let mut cpu_state = cpu::CpuState::Normal;
        let mut added_clock = 0;
        if self.excess_cpu_cycles == 0 {
            // this number doesn't mean anything
            // TODO: research on when to stop the CPU (maybe fixed number? block of code? other?)
            let cpu_cycles;
            let shell_reached;

            (cpu_cycles, cpu_state) = self.cpu.clock(&mut self.bus, 56);
            shell_reached = self.cpu.is_shell_reached();

            // handle fast booting and hijacking the bios to load exe
            if shell_reached && (self.config.fast_boot || self.exe_file.is_some()) {
                if let Some(exe_file) = &self.exe_file {
                    let (pc, gp, sp_fp) = self.bus.load_exe_in_memory(exe_file);

                    let regs = self.cpu.registers_mut();
                    println!(
                        "Loaded EXE {} into pc: {:08x}. gp: {:08x}, sp_fp: {:08x}",
                        exe_file.display(),
                        pc,
                        gp,
                        sp_fp
                    );

                    assert_ne!(pc, 0, "PC value cannot be zero");
                    regs.write(RegisterType::Pc, pc);

                    if gp != 0 {
                        regs.write(RegisterType::Gp, gp);
                    }
                    if sp_fp != 0 {
                        regs.write(RegisterType::Sp, sp_fp);
                        regs.write(RegisterType::Fp, sp_fp);
                    }
                } else if self.disk_available {
                    // we are either in a cd game or not, either way, skip the shell
                    let regs = self.cpu.registers_mut();
                    // return from the function
                    regs.write(RegisterType::Pc, regs.read(RegisterType::Ra));
                }
            }

            if cpu_cycles == 0 {
                return (0, cpu_state);
            }
            // the DMA is running of the CPU
            self.excess_cpu_cycles = cpu_cycles + self.bus.clock_dma();
            added_clock = self.excess_cpu_cycles;
        }

        let cpu_cycles_to_run = self.excess_cpu_cycles.min(MAX_CPU_CYCLES_TO_CLOCK);
        self.excess_cpu_cycles -= cpu_cycles_to_run;
        self.bus.clock_components(cpu_cycles_to_run);

        (added_clock, cpu_state)
    }

    /// Return `true` if the frame is finished, `false` otherwise.
    /// Return the CPU state.
    pub fn clock_based_on_audio(&mut self, max_clocks: u32) -> (bool, cpu::CpuState) {
        // sync the CPU clocks to the SPU so that the audio would be clearer.
        const CYCLES_PER_FRAME: u32 = 564480;

        let mut clocks = 0;

        while self.cpu_frame_cycles < CYCLES_PER_FRAME {
            let (added_clock, cpu_state) = self.common_clock();
            clocks += added_clock;
            self.cpu_frame_cycles += added_clock;

            if clocks >= max_clocks || cpu_state != cpu::CpuState::Normal {
                return (false, cpu_state);
            }
        }
        self.cpu_frame_cycles -= CYCLES_PER_FRAME;

        (true, cpu::CpuState::Normal)
    }

    /// Return `true` if the frame is finished, `false` otherwise.
    /// Return the CPU state.
    pub fn clock_based_on_video(&mut self, max_clocks: u32) -> (bool, cpu::CpuState) {
        let mut prev_vblank = self.bus.gpu().in_vblank();
        let mut current_vblank = prev_vblank;

        let mut clocks = 0;

        while !current_vblank || prev_vblank {
            let (added_clock, cpu_state) = self.common_clock();
            if cpu_state != cpu::CpuState::Normal {
                return (false, cpu_state);
            } else {
                clocks += added_clock;
                if clocks >= max_clocks {
                    return (false, cpu_state);
                }
            }

            prev_vblank = current_vblank;
            current_vblank = self.bus.gpu().in_vblank();
        }

        (true, cpu::CpuState::Normal)
    }

    pub fn clock_full_audio_frame(&mut self) -> cpu::CpuState {
        // sync the CPU clocks to the SPU so that the audio would be clearer.
        const CYCLES_PER_FRAME: u32 = 564480;

        let mut clocks = 0;
        while clocks < CYCLES_PER_FRAME {
            let (added_clock, cpu_state) = self.common_clock();
            clocks += added_clock;
            if cpu_state != cpu::CpuState::Normal {
                return cpu_state;
            }
        }

        cpu::CpuState::Normal
    }

    pub fn clock_full_video_frame(&mut self) -> cpu::CpuState {
        let mut prev_vblank = self.bus.gpu().in_vblank();
        let mut current_vblank = prev_vblank;

        while !current_vblank || prev_vblank {
            let cpu_state = self.common_clock().1;
            if cpu_state != cpu::CpuState::Normal {
                return cpu_state;
            }

            prev_vblank = current_vblank;
            current_vblank = self.bus.gpu().in_vblank();
        }

        cpu::CpuState::Normal
    }

    pub fn change_controller_key_state(&mut self, key: DigitalControllerKey, pressed: bool) {
        self.bus
            .controller_mem_card_mut()
            .change_controller_key_state(key, pressed);
    }

    pub fn change_cdrom_shell_open_state(&mut self, open: bool) {
        self.bus.cdrom_mut().change_cdrom_shell_open_state(open);
    }

    pub fn blit_to_front(
        &mut self,
        dest_image: Arc<Image>,
        full_vram: bool,
        in_future: Box<dyn GpuFuture>,
    ) -> Box<dyn GpuFuture> {
        self.bus
            .gpu_mut()
            .sync_gpu_and_blit_to_front(dest_image, full_vram, in_future)
    }

    pub fn take_audio_buffer(&mut self) -> Vec<f32> {
        self.bus.spu_mut().take_audio_buffer()
    }

    pub fn cpu(&mut self) -> &mut cpu::Cpu {
        &mut self.cpu
    }

    pub fn bus_read_u32(&mut self, addr: u32) -> Result<u32> {
        // make sure its aligned
        if addr % 4 != 0 {
            return Err("Unaligned memory access".to_string());
        }
        self.bus.read_u32(addr)
    }

    pub fn bus_read_u16(&mut self, addr: u32) -> Result<u16> {
        // make sure its aligned
        if addr % 2 != 0 {
            return Err("Unaligned memory access".to_string());
        }

        self.bus.read_u16(addr)
    }

    pub fn bus_read_u8(&mut self, addr: u32) -> Result<u8> {
        self.bus.read_u8(addr)
    }

    pub fn print_spu_state(&self) {
        self.bus.spu().print_state();
    }
}
