mod dma;
mod expansion_regions;
pub(crate) mod interrupts;
mod memory_control;
mod ram;

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use crate::gpu::{Device, Queue};
use byteorder::{ByteOrder, LittleEndian, ReadBytesExt};

use crate::cdrom::Cdrom;
use crate::controller_mem_card::ControllerAndMemoryCard;
use crate::cpu::CpuBusProvider;
use crate::gpu::Gpu;
use crate::mdec::Mdec;
use crate::spu::Spu;
use crate::timers::Timers;
use crate::{PsxConfig, PsxError};

use dma::Dma;
use expansion_regions::{ExpansionRegion1, ExpansionRegion2};
use interrupts::Interrupts;
use memory_control::{CacheControl, MemoryControl1, MemoryControl2};
use ram::{MainRam, Scratchpad};

pub type Result<T, E = String> = std::result::Result<T, E>;

/// A notion of a Busline, which is an interface to interact with memory mapped devices
/// it can be a memory, or other stuff.
///
/// Here we implement the behavior for `read/write` for each size of data (u32, u16, u8).
pub trait BusLine {
    fn read_u32(&mut self, addr: u32) -> Result<u32> {
        Err(format!(
            "{}: u32 read from {:08X}",
            std::any::type_name::<Self>(),
            addr
        ))
    }

    fn write_u32(&mut self, addr: u32, _data: u32) -> Result<()> {
        Err(format!(
            "{}: u32 write to {:08X}",
            std::any::type_name::<Self>(),
            addr
        ))
    }

    fn read_u16(&mut self, addr: u32) -> Result<u16> {
        Err(format!(
            "{}: u16 read from {:08X}",
            std::any::type_name::<Self>(),
            addr
        ))
    }
    fn write_u16(&mut self, addr: u32, _data: u16) -> Result<()> {
        Err(format!(
            "{}: u16 write to {:08X}",
            std::any::type_name::<Self>(),
            addr
        ))
    }

    fn read_u8(&mut self, addr: u32) -> Result<u8> {
        Err(format!(
            "{}: u8 read from {:08X}",
            std::any::type_name::<Self>(),
            addr
        ))
    }
    fn write_u8(&mut self, addr: u32, _data: u8) -> Result<()> {
        Err(format!(
            "{}: u8 write to {:08X}",
            std::any::type_name::<Self>(),
            addr
        ))
    }
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
        if self.read_u32(0x6f0c).unwrap() == 0x3C01A001
            && self.read_u32(0x6f14).unwrap() == 0xAC20B9B0
        {
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
        if self.read_u32(0x14330).unwrap() == 0x92200000
            && self.read_u32(0x14334).unwrap() == 0x10000047
            && self.read_u32(0x14338).unwrap() == 0x8fae0040
        {
            self.write_u32(0x14330, 0x00000000);
            self.write_u32(0x14334, 0x10000006);
            self.write_u32(0x14338, 0x00000000);
        }
    }
}

impl Bios {
    // TODO: produce a valid `Error` struct
    pub fn from_file<P: AsRef<Path>>(bios_file_path: P) -> Result<Self, PsxError> {
        let mut data = Vec::new();

        let mut file = File::open(bios_file_path).map_err(|_| PsxError::CouldNotLoadBios)?;

        file.read_to_end(&mut data)
            .map_err(|_| PsxError::CouldNotLoadBios)?;

        let mut s = Self { data };

        s.apply_patches();

        Ok(s)
    }

    pub fn read_u32(&self, addr: u32) -> Result<u32> {
        let index = (addr & 0xFFFFF) as usize;

        Ok(LittleEndian::read_u32(&self.data[index..index + 4]))
    }

    pub fn read_u16(&self, addr: u32) -> Result<u16> {
        let index = (addr & 0xFFFFF) as usize;

        Ok(LittleEndian::read_u16(&self.data[index..index + 4]))
    }

    pub fn read_u8(&self, addr: u32) -> Result<u8> {
        let index = (addr & 0xFFFFF) as usize;

        Ok(self.data[index])
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
    pub spu: Spu,
}

pub struct CpuBus {
    bios: Bios,
    mem_ctrl_1: MemoryControl1,
    mem_ctrl_2: MemoryControl2,
    cache_control: CacheControl,
    interrupts: Interrupts,
    controller_mem_card: ControllerAndMemoryCard,

    expansion_region_1: ExpansionRegion1,
    expansion_region_2: ExpansionRegion2,

    timers: Timers,

    dma: Dma,
    dma_bus: DmaBus,

    scratchpad: Scratchpad,
    config: PsxConfig,
}

impl CpuBus {
    pub fn new<DiskPath: AsRef<Path>>(
        bios: Bios,
        disk_file: Option<DiskPath>,
        config: PsxConfig,
        device: Arc<Device>,
        queue: Arc<Queue>,
    ) -> Result<Self, PsxError> {
        let mut s = Self {
            bios,
            mem_ctrl_1: MemoryControl1::default(),
            mem_ctrl_2: MemoryControl2::default(),
            cache_control: CacheControl::default(),
            interrupts: Interrupts::default(),
            controller_mem_card: ControllerAndMemoryCard::default(),

            expansion_region_1: ExpansionRegion1::default(),
            expansion_region_2: ExpansionRegion2::new(config),
            dma: Dma::default(),

            timers: Timers::default(),

            dma_bus: DmaBus {
                cdrom: Cdrom::default(),
                gpu: Gpu::new(device, queue),
                main_ram: MainRam::default(),
                mdec: Mdec::default(),
                spu: Spu::default(),
            },

            scratchpad: Scratchpad::default(),
            config,
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
                "cue" => s.dma_bus.cdrom.set_cue_file(disk_file)?,
                _ => {
                    return Err(PsxError::DiskTypeNotSupported);
                }
            }
        }

        Ok(s)
    }

    pub fn reset(&mut self) {
        self.mem_ctrl_1 = MemoryControl1::default();
        self.mem_ctrl_2 = MemoryControl2::default();
        self.cache_control = CacheControl::default();
        self.interrupts = Interrupts::default();
        self.controller_mem_card = ControllerAndMemoryCard::default();

        self.expansion_region_1 = ExpansionRegion1::default();
        self.expansion_region_2 = ExpansionRegion2::new(self.config);
        self.dma = Dma::default();

        self.timers = Timers::default();

        self.dma_bus.cdrom.reset();
        self.dma_bus.gpu.reset();
        self.dma_bus.main_ram = MainRam::default();
        self.dma_bus.mdec = Mdec::default();
        self.dma_bus.spu = Spu::default();

        self.scratchpad = Scratchpad::default();
    }

    pub fn gpu(&self) -> &Gpu {
        &self.dma_bus.gpu
    }

    pub fn gpu_mut(&mut self) -> &mut Gpu {
        &mut self.dma_bus.gpu
    }

    pub fn controller_mem_card_mut(&mut self) -> &mut ControllerAndMemoryCard {
        &mut self.controller_mem_card
    }

    pub fn spu(&self) -> &Spu {
        &self.dma_bus.spu
    }

    pub fn spu_mut(&mut self) -> &mut Spu {
        &mut self.dma_bus.spu
    }

    pub fn cdrom_mut(&mut self) -> &mut Cdrom {
        &mut self.dma_bus.cdrom
    }
}

impl CpuBus {
    // TODO: handle errors
    //
    /// Returns the metadata of the loaded exe
    pub fn load_exe_in_memory<P: AsRef<Path>>(&mut self, exe_file_path: P) -> (u32, u32, u32) {
        let mut file = File::open(exe_file_path).unwrap();
        let mut magic = [0; 8];
        let mut data = Vec::new();

        file.read_exact(&mut magic).unwrap();
        assert!(&magic == b"PS-X EXE");
        // bytes from 0x8 to 0xF
        assert!(file.read_u64::<LittleEndian>().unwrap() == 0);

        let initial_pc = file.read_u32::<LittleEndian>().unwrap();
        let initial_gp = file.read_u32::<LittleEndian>().unwrap();
        let destination = file.read_u32::<LittleEndian>().unwrap();
        let file_size = file.read_u32::<LittleEndian>().unwrap();
        let _data_section_start = file.read_u32::<LittleEndian>().unwrap();
        let _data_section_size = file.read_u32::<LittleEndian>().unwrap();
        let _bss_section_start = file.read_u32::<LittleEndian>().unwrap();
        let _bss_section_size = file.read_u32::<LittleEndian>().unwrap();
        let mut initial_sp_fp = file.read_u32::<LittleEndian>().unwrap();
        initial_sp_fp += file.read_u32::<LittleEndian>().unwrap();
        // ascii marker and zero filled data
        file.seek(SeekFrom::Start(0x800)).unwrap();

        file.read_to_end(&mut data).unwrap();

        assert!(data.len() == file_size as usize);

        // put the data at the correct location in ram
        self.dma_bus
            .main_ram
            .put_at_address(&data, destination & 0x1FFFFF);

        (initial_pc, initial_gp, initial_sp_fp)
    }

    /// Since DMA is running using the CPU resources, we should run it and
    /// treat the cycles consumed by it as if they were running from the CPU
    pub fn clock_dma(&mut self) -> u32 {
        self.dma.clock_dma(&mut self.dma_bus, &mut self.interrupts)
    }

    pub fn clock_components(&mut self, cpu_cycles: u32) {
        let (dot_clocks, hblank_clock) = self.dma_bus.gpu.clock(&mut self.interrupts, cpu_cycles);

        self.dma_bus.spu.clock(&mut self.interrupts, cpu_cycles);

        // controller and mem card
        self.controller_mem_card
            .clock(&mut self.interrupts, cpu_cycles);

        // cdrom (takes SPU to be able to send cdrom audio to the mixer)
        self.dma_bus
            .cdrom
            .clock(&mut self.interrupts, &mut self.dma_bus.spu, cpu_cycles);

        // timers
        self.timers.clock_from_system(cpu_cycles);
        if hblank_clock {
            self.timers.clock_from_hblank();
        }
        self.timers.clock_from_gpu_dot(dot_clocks);
        // interrupts for the timers
        self.timers.handle_interrupts(&mut self.interrupts);
    }

    // implement the PSX memory map
    // Note that `addr >= 0xFFFE0000` point to the cache control registers and isn't changed
    fn map_address(&self, addr: u32) -> Result<u32> {
        let region = addr >> 29;
        const MASK_512M: u32 = 0x1FFFFFFF;

        match region {
            // KUSEG mirror of KSEG0/KSEG1
            0 => Ok(addr & MASK_512M),
            1..=3 => Err(String::from("Accessing bottom 1.5G of KUSEG")),
            // KSEG0
            4 => Ok(addr & MASK_512M),
            // KSEG1
            5 => {
                if (0xBF800000..0xBF801000).contains(&addr) {
                    Err(String::from("Cannot access scratchpad from KSEG1"))
                } else {
                    Ok(addr & MASK_512M)
                }
            }
            // KSEG2
            7 if addr >= 0xFFFE0000 => Ok(addr), // no change
            6 | 7 => Err(String::from(
                "KSEG2 has only the cache control registers at 0xFFFE0000",
            )),
            _ => unreachable!(),
        }
    }
}

impl BusLine for CpuBus {
    fn read_u32(&mut self, addr: u32) -> Result<u32> {
        assert!(addr % 4 == 0, "unalligned u32 read");
        let addr = self.map_address(addr)?;

        match addr {
            // TODO: implement I-cache isolation properly
            0x00000000..=0x007FFFFF => self.dma_bus.main_ram.read_u32(addr & 0x1FFFFF),
            0x1FC00000..=0x1FC80000 => self.bios.read_u32(addr),
            0x1F800000..=0x1F8003FF => self.scratchpad.read_u32(addr & 0x3FF),
            0x1F801000..=0x1F801020 => self.mem_ctrl_1.read_u32(addr),
            0x1F801044..=0x1F80104F => self.controller_mem_card.read_u32(addr & 0xF),
            0x1F801060 => self.mem_ctrl_2.read_u32(addr),
            0x1F801070..=0x1F801077 => self.interrupts.read_u32(addr & 0xF),
            0x1F801080..=0x1F8010FC => self.dma.read_u32(addr & 0xFF),
            0x1F801100..=0x1F80112F => self.timers.read_u32(addr & 0xFF),
            0x1F801810..=0x1F801814 => self.dma_bus.gpu.read_u32(addr & 0xF),
            0x1F801820..=0x1F801824 => self.dma_bus.mdec.read_u32(addr & 0xF),
            0x1F801C00..=0x1F801FFC => self.dma_bus.spu.read_u32(addr & 0x3FF),
            0x1F802000..=0x1F80208F => self.expansion_region_2.read_u32(addr & 0xFF),
            0xFFFE0130 => self.cache_control.read_u32(addr),
            _ => Err(format!("MainBus: u32 read from {:08X}", addr)),
        }
    }

    fn write_u32(&mut self, addr: u32, data: u32) -> Result<()> {
        assert!(addr % 4 == 0, "unalligned u32 write");
        let addr = self.map_address(addr)?;

        match addr {
            0x00000000..=0x007FFFFF => self.dma_bus.main_ram.write_u32(addr & 0x1FFFFF, data),
            0x1F800000..=0x1F8003FF => self.scratchpad.write_u32(addr & 0x3FF, data),
            0x1F801000..=0x1F801020 => self.mem_ctrl_1.write_u32(addr, data),
            0x1F801060 => self.mem_ctrl_2.write_u32(addr, data),
            0x1F801070..=0x1F801077 => self.interrupts.write_u32(addr & 0xF, data),
            0x1F801080..=0x1F8010FC => self.dma.write_u32(addr & 0xFF, data),
            0x1F801100..=0x1F80112F => self.timers.write_u32(addr & 0xFF, data),
            0x1F801810..=0x1F801814 => self.dma_bus.gpu.write_u32(addr & 0xF, data),
            0x1F801820..=0x1F801824 => self.dma_bus.mdec.write_u32(addr & 0xF, data),
            0x1F801C00..=0x1F801FFC => self.dma_bus.spu.write_u32(addr & 0x3FF, data),
            0x1F802000..=0x1F80208F => self.expansion_region_2.write_u32(addr & 0xFF, data),
            0xFFFE0130 => self.cache_control.write_u32(addr, data),
            _ => Err(format!("MainBus: u32 write to {:08X}", addr)),
        }
    }

    fn read_u16(&mut self, addr: u32) -> Result<u16> {
        assert!(addr % 2 == 0, "unalligned u16 read");
        let addr = self.map_address(addr)?;

        match addr {
            0x00000000..=0x007FFFFF => self.dma_bus.main_ram.read_u16(addr & 0x1FFFFF),
            0x1F800000..=0x1F8003FF => self.scratchpad.read_u16(addr & 0x3FF),
            0x1F801044..=0x1F80104F => self.controller_mem_card.read_u16(addr & 0xF),
            0x1F801070..=0x1F801077 => self.interrupts.read_u16(addr & 0xF),
            0x1F801100..=0x1F80112F => self.timers.read_u16(addr & 0xFF),
            0x1F801C00..=0x1F801FFC => self.dma_bus.spu.read_u16(addr & 0x3FF),
            0x1FC00000..=0x1FC80000 => self.bios.read_u16(addr),
            0x1F802000..=0x1F80208F => self.expansion_region_2.read_u16(addr & 0xFF),
            _ => Err(format!("u16 read from {:08X}", addr)),
        }
    }

    fn write_u16(&mut self, addr: u32, data: u16) -> Result<()> {
        assert!(addr % 2 == 0, "unalligned u16 write");
        let addr = self.map_address(addr)?;

        match addr {
            0x00000000..=0x007FFFFF => self.dma_bus.main_ram.write_u16(addr & 0x1FFFFF, data),
            0x1F800000..=0x1F8003FF => self.scratchpad.write_u16(addr & 0x3FF, data),
            0x1F801048..=0x1F80104F => self.controller_mem_card.write_u16(addr & 0xF, data),
            0x1F801070..=0x1F801077 => self.interrupts.write_u16(addr & 0xF, data),
            0x1F801100..=0x1F80112F => self.timers.write_u16(addr & 0xFF, data),
            0x1F801C00..=0x1F801FFC => self.dma_bus.spu.write_u16(addr & 0x3FF, data),
            0x1F802000..=0x1F80208F => self.expansion_region_2.write_u16(addr & 0xFF, data),
            _ => Err(format!("u16 write to {:08X}", addr)),
        }
    }
    fn read_u8(&mut self, addr: u32) -> Result<u8> {
        let addr = self.map_address(addr)?;

        match addr {
            0x00000000..=0x007FFFFF => self.dma_bus.main_ram.read_u8(addr & 0x1FFFFF),
            0x1F800000..=0x1F8003FF => self.scratchpad.read_u8(addr & 0x3FF),
            0x1F801040 => self.controller_mem_card.read_u8(addr & 0xF),
            0x1F000000..=0x1F080000 => self.expansion_region_1.read_u8(addr & 0xFFFFF),
            0x1F801080..=0x1F8010FF => self.dma.read_u8(addr & 0xFF),
            0x1F801800..=0x1F801803 => self.dma_bus.cdrom.read_u8(addr & 3),
            0x1F802000..=0x1F80208F => self.expansion_region_2.read_u8(addr & 0xFF),
            0x1FC00000..=0x1FC80000 => self.bios.read_u8(addr),
            _ => Err(format!("u8 read from {:08X}", addr)),
        }
    }

    fn write_u8(&mut self, addr: u32, data: u8) -> Result<()> {
        let addr = self.map_address(addr)?;

        match addr {
            0x00000000..=0x007FFFFF => self.dma_bus.main_ram.write_u8(addr & 0x1FFFFF, data),
            0x1F800000..=0x1F8003FF => self.scratchpad.write_u8(addr & 0x3FF, data),
            0x1F801040 => self.controller_mem_card.write_u8(addr & 0xF, data),
            0x1F000000..=0x1F080000 => self.expansion_region_1.write_u8(addr & 0xFFFFF, data),
            0x1F801080..=0x1F8010FF => self.dma.write_u8(addr & 0xFF, data),
            0x1F801800..=0x1F801803 => self.dma_bus.cdrom.write_u8(addr & 3, data),
            0x1F802000..=0x1F80208F => self.expansion_region_2.write_u8(addr & 0xFF, data),
            _ => Err(format!("u8 write to {:08X}", addr)),
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
