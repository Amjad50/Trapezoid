mod dma;
mod expansion_regions;
pub(crate) mod interrupts;
mod memory_control;
mod ram;

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use byteorder::{ByteOrder, LittleEndian, ReadBytesExt};
use vulkano::device::{Device, Queue};

use crate::cdrom::Cdrom;
use crate::controller_mem_card::ControllerAndMemoryCard;
use crate::cpu::CpuBusProvider;
use crate::gpu::Gpu;
use crate::mdec::Mdec;
use crate::spu::SpuRegisters;
use crate::timers::Timers;

use dma::Dma;
use expansion_regions::{ExpansionRegion1, ExpansionRegion2};
use interrupts::Interrupts;
use memory_control::{CacheControl, MemoryControl1, MemoryControl2};
use ram::{MainRam, Scratchpad};

pub trait BusLine {
    fn read_u32(&mut self, addr: u32) -> u32;
    fn write_u32(&mut self, addr: u32, data: u32);

    fn read_u16(&mut self, addr: u32) -> u16;
    fn write_u16(&mut self, addr: u32, data: u16);

    fn read_u8(&mut self, addr: u32) -> u8;
    fn write_u8(&mut self, addr: u32, data: u8);
}

pub struct Bios {
    data: Vec<u8>,
}

impl Bios {
    fn write_u32(&mut self, addr: u32, data: u32) {
        let index = (addr & 0xFFFFF) as usize;

        LittleEndian::write_u32(&mut self.data[index..index + 4], data)
    }

    fn apply_patches(&mut self) {
        // patch to support TTY
        // the BIOS by default hardcode disable the TTY driver, here we change it
        // from writing 0 to 1 in order to enable the driver load
        if self.read_u32(0x6f0c) == 0x3C01A001 && self.read_u32(0x6f14) == 0xAC20B9B0 {
            self.write_u32(0x6f0c, 0x34010001);
            self.write_u32(0x6f14, 0xAF81A9C0);
        }
        // patch to fix a bug where the cursor of the controller is blinking
        //
        // The problem is that the BIOS does this when getting the digital switches data:
        //     while (I_STAT & 0x80 == 0) {
        //         if (JOY_STAT & 2 != 0) {
        //             goto save_input_and_continue;
        //         }
        //     }
        //     while (JOY_STAT & 2 == 0) {}
        //     // return value for "controller not connected"
        //     return 0xFFFF;
        //
        // Which will save the input and continue *only and only if* it read the
        // `controller_and_mem_card` interrupt from `I_STAT` first then read the
        // `RX_FIFO_NOT_EMPTY` from `JOY_STAT`. If it read it the other way around
        // (meaning that the transfer finished just after the read of `JOY_STAT`
        // inside the loop, then it will report as `controller not connected`.
        //
        // This problem happens in some frames and results in blinking cursor.
        //
        // The patch fixes this issue by modifing the jump after the first loop
        // to the `save_input_and_continue` address. It was also tested, switches
        // in the controller works without any problems and the BIOS can read
        // the keys, only the blinking is fixed.
        if self.read_u32(0x14330) == 0x92200000
            && self.read_u32(0x14334) == 0x10000047
            && self.read_u32(0x14338) == 0x8fae0040
        {
            self.write_u32(0x14330, 0x00000000);
            self.write_u32(0x14334, 0x10000006);
            self.write_u32(0x14338, 0x00000000);
        }
    }
}

impl Bios {
    // TODO: produce a valid `Error` struct
    pub fn from_file<P: AsRef<Path>>(bios_file_path: P) -> Result<Self, ()> {
        let mut data = Vec::new();

        let mut file = File::open(bios_file_path).map_err(|_| ())?;

        file.read_to_end(&mut data).map_err(|_| ())?;

        let mut s = Self { data };

        s.apply_patches();

        Ok(s)
    }

    pub fn read_u32(&self, addr: u32) -> u32 {
        let index = (addr & 0xFFFFF) as usize;

        LittleEndian::read_u32(&self.data[index..index + 4])
    }

    pub fn read_u16(&self, addr: u32) -> u16 {
        let index = (addr & 0xFFFFF) as usize;

        LittleEndian::read_u16(&self.data[index..index + 4])
    }

    pub fn read_u8(&self, addr: u32) -> u8 {
        let index = (addr & 0xFFFFF) as usize;

        self.data[index]
    }
}

/// A structure that holds the elements of the emulator that the DMA can access
///
/// These are the elements access by the channels:
/// 0 & 1- MDEC
/// 2- GPU
/// 3- CDROM
/// 4- SPU
/// 5- PIO
/// 6- OTC (GPU)
///
/// And also the main ram to write/read to/from.
///
/// The reason for this design, is to be able to pass this structure `&mut`
/// to `Dma` without problems of double mut.
struct DmaBus {
    pub main_ram: MainRam,
    pub cdrom: Cdrom,
    pub gpu: Gpu,
    pub mdec: Mdec,
}

pub struct CpuBus {
    bios: Bios,
    mem_ctrl_1: MemoryControl1,
    mem_ctrl_2: MemoryControl2,
    cache_control: CacheControl,
    interrupts: Interrupts,
    controller_mem_card: ControllerAndMemoryCard,

    spu_registers: SpuRegisters,

    expansion_region_1: ExpansionRegion1,
    expansion_region_2: ExpansionRegion2,

    timers: Timers,

    dma: Dma,
    dma_bus: DmaBus,

    scratchpad: Scratchpad,
}

impl CpuBus {
    pub fn new<DiskPath: AsRef<Path>>(
        bios: Bios,
        disk_file: Option<DiskPath>,
        device: Arc<Device>,
        queue: Arc<Queue>,
    ) -> Self {
        let mut s = Self {
            bios,
            mem_ctrl_1: MemoryControl1::default(),
            mem_ctrl_2: MemoryControl2::default(),
            cache_control: CacheControl::default(),
            interrupts: Interrupts::default(),
            controller_mem_card: ControllerAndMemoryCard::default(),

            spu_registers: SpuRegisters::default(),
            expansion_region_1: ExpansionRegion1::default(),
            expansion_region_2: ExpansionRegion2::default(),
            dma: Dma::default(),

            timers: Timers::default(),

            dma_bus: DmaBus {
                cdrom: Cdrom::default(),
                gpu: Gpu::new(device, queue),
                main_ram: MainRam::default(),
                mdec: Mdec::default(),
            },

            scratchpad: Scratchpad::default(),
        };

        // TODO: handle errors in loading
        if let Some(disk_file) = disk_file {
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
                "exe" => s.load_exe_file(disk_file),
                "cue" => s.dma_bus.cdrom.set_cue_file(disk_file),
                _ => todo!("Only exe is supported now"),
            }
        }

        s
    }

    pub fn gpu(&self) -> &Gpu {
        &self.dma_bus.gpu
    }

    pub fn controller_mem_card_mut(&mut self) -> &mut ControllerAndMemoryCard {
        &mut self.controller_mem_card
    }
}

impl CpuBus {
    // TODO: handle errors
    fn load_exe_file<P: AsRef<Path>>(&mut self, exe_file_path: P) {
        let mut file = File::open(exe_file_path).unwrap();
        let mut magic = [0; 8];
        let initial_pc;
        let initial_gp;
        let destination;
        let file_size;
        let _data_section_start;
        let _data_section_size;
        let _bss_section_start;
        let _bss_section_size;
        let mut initial_sp_fp;
        let mut data = Vec::new();

        file.read_exact(&mut magic).unwrap();
        assert!(&magic == b"PS-X EXE");
        // bytes from 0x8 to 0xF
        assert!(file.read_u64::<LittleEndian>().unwrap() == 0);

        initial_pc = file.read_u32::<LittleEndian>().unwrap();
        initial_gp = file.read_u32::<LittleEndian>().unwrap();
        destination = file.read_u32::<LittleEndian>().unwrap();
        file_size = file.read_u32::<LittleEndian>().unwrap();
        _data_section_start = file.read_u32::<LittleEndian>().unwrap();
        _data_section_size = file.read_u32::<LittleEndian>().unwrap();
        _bss_section_start = file.read_u32::<LittleEndian>().unwrap();
        _bss_section_size = file.read_u32::<LittleEndian>().unwrap();
        initial_sp_fp = file.read_u32::<LittleEndian>().unwrap();
        initial_sp_fp += file.read_u32::<LittleEndian>().unwrap();
        // ascii marker and zero filled data
        file.seek(SeekFrom::Start(0x800)).unwrap();

        file.read_to_end(&mut data).unwrap();

        assert!(data.len() == file_size as usize);

        // put the data at the correct location in ram
        self.dma_bus
            .main_ram
            .put_at_address(&data, destination & 0x1FFFFF);

        let sp_fp_mask = (initial_sp_fp != 0) as u32 * 0xFFFFFFFF;
        let data = [
            0x3C080000 | initial_pc >> 16,
            0x35080000 | initial_pc & 0xFFFF,
            0x3C1C0000 | initial_gp >> 16,
            0x379C0000 | initial_gp & 0xFFFF,
            // if sp_fp is zero, these will be NOP instructions
            sp_fp_mask & (0x3C1D0000 | initial_sp_fp >> 16),
            sp_fp_mask & (0x37BD0000 | initial_sp_fp & 0xFFFF),
            sp_fp_mask & (0x3C1E0000 | initial_sp_fp >> 16),
            sp_fp_mask & (0x37DE0000 | initial_sp_fp & 0xFFFF),
            0x01000008,
            0x00000000,
        ];

        // patch the bios's `LoadRunShell` to run the exe instead
        // This patch method was taken from https://github.com/BluestormDNA/ProjectPSX
        for (i, &d) in data.iter().enumerate() {
            let offset = i * 4;
            LittleEndian::write_u32(&mut self.bios.data[0x6FF0 + offset..0x6FF0 + offset + 4], d);
        }
    }

    /// Since DMA is running using the CPU resources, we should run it and
    /// treat the cycles consumed by it as if they were running from the CPU
    pub fn clock_dma(&mut self) -> u32 {
        self.dma.clock_dma(&mut self.dma_bus, &mut self.interrupts)
    }

    pub fn clock_components(&mut self, cpu_cycles: u32) {
        // almost 2 GPU clocks per 1 CPU
        let (dot_clocks, hblank_clock) =
            self.dma_bus.gpu.clock(&mut self.interrupts, cpu_cycles * 2);

        // controller and mem card
        self.controller_mem_card
            .clock(&mut self.interrupts, cpu_cycles);

        // cdrom
        self.dma_bus.cdrom.clock(&mut self.interrupts, cpu_cycles);

        // timers
        self.timers.clock_from_system(cpu_cycles);
        if hblank_clock {
            self.timers.clock_from_hblank();
        }
        self.timers.clock_from_gpu_dot(dot_clocks);
        // interrupts for the timers
        self.timers.handle_interrupts(&mut self.interrupts);
    }
}

impl BusLine for CpuBus {
    fn read_u32(&mut self, addr: u32) -> u32 {
        assert!(addr % 4 == 0, "unalligned u32 read");
        match addr {
            // TODO: implement I-cache isolation properly
            0x00000000..=0x007FFFFF => self.dma_bus.main_ram.read_u32(addr & 0x1FFFFF),
            // TODO: implement mirroring in a better way (with cache as well maybe)
            0x80000000..=0x807FFFFF => self.dma_bus.main_ram.read_u32(addr & 0x1FFFFF),
            0xA0000000..=0xA07FFFFF => self.dma_bus.main_ram.read_u32(addr & 0x1FFFFF),
            0xBFC00000..=0xBFC80000 => self.bios.read_u32(addr),
            0x1F800000..=0x1F8003FF => self.scratchpad.read_u32(addr & 0x3FF),
            0x1F801000..=0x1F801020 => self.mem_ctrl_1.read_u32(addr),
            0x1F801060 => self.mem_ctrl_2.read_u32(addr),
            0x1F801070..=0x1F801077 => self.interrupts.read_u32(addr & 0xF),
            0x1F801080..=0x1F8010FC => self.dma.read_u32(addr & 0xFF),
            0x1F801100..=0x1F80112F => self.timers.read_u32(addr & 0xFF),
            0x1F801810..=0x1F801814 => self.dma_bus.gpu.read_u32(addr & 0xF),
            0x1F801820..=0x1F801824 => self.dma_bus.mdec.read_u32(addr & 0xF),
            0x1F801C00..=0x1F802000 => self.spu_registers.read_u32((addr & 0xFFF) - 0xC00),
            0xFFFE0130 => self.cache_control.read_u32(addr),
            _ => {
                todo!("u32 read from {:08X}", addr)
            }
        }
    }

    fn write_u32(&mut self, addr: u32, data: u32) {
        assert!(addr % 4 == 0, "unalligned u32 write");

        match addr {
            0x00000000..=0x007FFFFF => self.dma_bus.main_ram.write_u32(addr & 0x1FFFFF, data),
            0x80000000..=0x807FFFFF => self.dma_bus.main_ram.write_u32(addr & 0x1FFFFF, data),
            0xA0000000..=0xA07FFFFF => self.dma_bus.main_ram.write_u32(addr & 0x1FFFFF, data),
            0x1F800000..=0x1F8003FF => self.scratchpad.write_u32(addr & 0x3FF, data),
            0x1F801000..=0x1F801020 => self.mem_ctrl_1.write_u32(addr, data),
            0x1F801060 => self.mem_ctrl_2.write_u32(addr, data),
            0x1F801070..=0x1F801077 => self.interrupts.write_u32(addr & 0xF, data),
            0x1F801080..=0x1F8010FC => self.dma.write_u32(addr & 0xFF, data),
            0x1F801100..=0x1F80112F => self.timers.write_u32(addr & 0xFF, data),
            0x1F801810..=0x1F801814 => self.dma_bus.gpu.write_u32(addr & 0xF, data),
            0x1F801820..=0x1F801824 => self.dma_bus.mdec.write_u32(addr & 0xF, data),
            0x1F801C00..=0x1F802000 => self.spu_registers.write_u32((addr & 0xFFF) - 0xC00, data),
            0xFFFE0130 => self.cache_control.write_u32(addr, data),
            _ => {
                todo!("u32 write to {:08X}", addr)
            }
        }
    }

    fn read_u16(&mut self, addr: u32) -> u16 {
        assert!(addr % 2 == 0, "unalligned u16 read");

        match addr {
            0x00000000..=0x007FFFFF => self.dma_bus.main_ram.read_u16(addr & 0x1FFFFF),
            0x80000000..=0x807FFFFF => self.dma_bus.main_ram.read_u16(addr & 0x1FFFFF),
            0xA0000000..=0xA07FFFFF => self.dma_bus.main_ram.read_u16(addr & 0x1FFFFF),

            0x1F800000..=0x1F8003FF => self.scratchpad.read_u16(addr & 0x3FF),
            0x1F801044..=0x1F80104F => self.controller_mem_card.read_u16(addr & 0xF),
            0x1F801070..=0x1F801077 => self.interrupts.read_u16(addr & 0xF),
            0x1F801100..=0x1F80112F => self.timers.read_u16(addr & 0xFF),
            0x1F801C00..=0x1F802000 => self.spu_registers.read_u16((addr & 0xFFF) - 0xC00),
            0xBFC00000..=0xBFC80000 => self.bios.read_u16(addr),
            _ => {
                todo!("u16 read from {:08X}", addr)
            }
        }
    }

    fn write_u16(&mut self, addr: u32, data: u16) {
        assert!(addr % 2 == 0, "unalligned u16 write");

        match addr {
            0x00000000..=0x007FFFFF => self.dma_bus.main_ram.write_u16(addr & 0x1FFFFF, data),
            0x80000000..=0x807FFFFF => self.dma_bus.main_ram.write_u16(addr & 0x1FFFFF, data),
            0xA0000000..=0xA07FFFFF => self.dma_bus.main_ram.write_u16(addr & 0x1FFFFF, data),

            0x1F800000..=0x1F8003FF => self.scratchpad.write_u16(addr & 0x3FF, data),
            0x1F801048..=0x1F80104F => self.controller_mem_card.write_u16(addr & 0xF, data),
            0x1F801070..=0x1F801077 => self.interrupts.write_u16(addr & 0xF, data),
            0x1F801100..=0x1F80112F => self.timers.write_u16(addr & 0xFF, data),
            0x1F801C00..=0x1F802000 => self.spu_registers.write_u16((addr & 0xFFF) - 0xC00, data),
            _ => {
                todo!("u16 write to {:08X}", addr)
            }
        }
    }
    fn read_u8(&mut self, addr: u32) -> u8 {
        match addr {
            0x00000000..=0x007FFFFF => self.dma_bus.main_ram.read_u8(addr & 0x1FFFFF),
            0x80000000..=0x807FFFFF => self.dma_bus.main_ram.read_u8(addr & 0x1FFFFF),
            0xA0000000..=0xA07FFFFF => self.dma_bus.main_ram.read_u8(addr & 0x1FFFFF),

            0x1F800000..=0x1F8003FF => self.scratchpad.read_u8(addr & 0x3FF),
            0x1F801040 => self.controller_mem_card.read_u8(addr & 0xF),
            0x1F000000..=0x1F080000 => self.expansion_region_1.read_u8(addr & 0xFFFFF),
            0x1F801800..=0x1F801803 => self.dma_bus.cdrom.read_u8(addr & 3),
            0x1F802000..=0x1F802080 => self.expansion_region_2.read_u8(addr & 0xFF),
            0xBFC00000..=0xBFC80000 => self.bios.read_u8(addr),
            _ => {
                todo!("u8 read from {:08X}", addr)
            }
        }
    }

    fn write_u8(&mut self, addr: u32, data: u8) {
        match addr {
            0x00000000..=0x007FFFFF => self.dma_bus.main_ram.write_u8(addr & 0x1FFFFF, data),
            0x80000000..=0x807FFFFF => self.dma_bus.main_ram.write_u8(addr & 0x1FFFFF, data),
            0xA0000000..=0xA07FFFFF => self.dma_bus.main_ram.write_u8(addr & 0x1FFFFF, data),

            0x1F800000..=0x1F8003FF => self.scratchpad.write_u8(addr & 0x3FF, data),
            0x1F801040 => self.controller_mem_card.write_u8(addr & 0xF, data),
            0x1F000000..=0x1F080000 => self.expansion_region_1.write_u8(addr & 0xFFFFF, data),
            0x1F801800..=0x1F801803 => self.dma_bus.cdrom.write_u8(addr & 3, data),
            0x1F802000..=0x1F802080 => self.expansion_region_2.write_u8(addr & 0xFF, data),
            _ => {
                todo!("u8 write to {:08X}", addr)
            }
        }
    }
}

impl CpuBusProvider for CpuBus {
    fn pending_interrupts(&self) -> bool {
        self.interrupts.pending_interrupts()
    }

    fn should_run_dma(&self) -> bool {
        self.dma.needs_to_run()
    }
}
