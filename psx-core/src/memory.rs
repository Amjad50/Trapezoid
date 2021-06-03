mod dma;
mod expansion_regions;
mod interrupts;
mod memory_control;
mod ram;

use std::fs::File;
use std::io::Read;
use std::path::Path;

use byteorder::{ByteOrder, LittleEndian};

use crate::cpu::CpuBusProvider;
use crate::gpu::{GlContext, Gpu};
use crate::spu::SpuRegisters;
use crate::timers::Timers;

use dma::Dma;
use expansion_regions::{ExpansionRegion1, ExpansionRegion2};
use interrupts::Interrupts;
use memory_control::{CacheControl, MemoryControl1, MemoryControl2};
use ram::MainRam;

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
    // TODO: produce a valid `Error` struct
    pub fn from_file<P: AsRef<Path>>(bios_file_path: P) -> Result<Self, ()> {
        let mut data = Vec::new();

        let mut file = File::open(bios_file_path).map_err(|_| ())?;

        file.read_to_end(&mut data).map_err(|_| ())?;

        Ok(Self { data })
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
    pub gpu: Gpu,
}

pub struct CpuBus {
    bios: Bios,
    mem_ctrl_1: MemoryControl1,
    mem_ctrl_2: MemoryControl2,
    cache_control: CacheControl,
    interrupts: Interrupts,

    spu_registers: SpuRegisters,

    expansion_region_1: ExpansionRegion1,
    expansion_region_2: ExpansionRegion2,

    timers: Timers,

    dma: Dma,
    dma_bus: DmaBus,
}

impl CpuBus {
    pub fn new(bios: Bios, gl_context: GlContext) -> Self {
        Self {
            bios,
            mem_ctrl_1: MemoryControl1::default(),
            mem_ctrl_2: MemoryControl2::default(),
            cache_control: CacheControl::default(),
            interrupts: Interrupts::default(),
            spu_registers: SpuRegisters::default(),
            expansion_region_1: ExpansionRegion1::default(),
            expansion_region_2: ExpansionRegion2::default(),
            dma: Dma::default(),

            timers: Timers::default(),

            dma_bus: DmaBus {
                main_ram: MainRam::default(),
                gpu: Gpu::new(gl_context),
            },
        }
    }

    pub fn gpu(&self) -> &Gpu {
        &self.dma_bus.gpu
    }
}

impl BusLine for CpuBus {
    fn read_u32(&mut self, addr: u32) -> u32 {
        assert!(addr % 4 == 0, "unalligned u32 read");
        // TODO: handle Cpu timing better (this should clock for at least once
        //  for every instruction)
        self.dma.clock_dma(&mut self.dma_bus, &mut self.interrupts);
        // almost 2 GPU clocks per 1 CPU
        self.dma_bus.gpu.clock();
        self.dma_bus.gpu.clock();

        match addr {
            // TODO: implement I-cache isolation properly
            0x00000000..=0x00200000 => self.dma_bus.main_ram.read_u32(addr),
            // TODO: implement mirroring in a better way (with cache as well maybe)
            0x80000000..=0x80200000 => self.dma_bus.main_ram.read_u32(addr & 0xFFFFFF),
            0xA0000000..=0xA0200000 => self.dma_bus.main_ram.read_u32(addr & 0xFFFFFF),
            0xBFC00000..=0xBFC80000 => self.bios.read_u32(addr),
            0x1F801000..=0x1F801020 => self.mem_ctrl_1.read_u32(addr),
            0x1F801060 => self.mem_ctrl_2.read_u32(addr),
            0x1F801070..=0x1F801077 => self.interrupts.read_u32(addr & 0xF),
            0x1F801080..=0x1F8010FC => self.dma.read_u32(addr & 0xFF),
            0x1F801100..=0x1F80112F => self.timers.read_u32(addr & 0xFF),
            0x1F801810..=0x1F801814 => self.dma_bus.gpu.read_u32(addr & 0xF),
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
            0x00000000..=0x00200000 => self.dma_bus.main_ram.write_u32(addr, data),
            0x80000000..=0x80200000 => self.dma_bus.main_ram.write_u32(addr & 0xFFFFFF, data),
            0xA0000000..=0xA0200000 => self.dma_bus.main_ram.write_u32(addr & 0xFFFFFF, data),
            0x1F801000..=0x1F801020 => self.mem_ctrl_1.write_u32(addr, data),
            0x1F801060 => self.mem_ctrl_2.write_u32(addr, data),
            0x1F801070..=0x1F801077 => self.interrupts.write_u32(addr & 0xF, data),
            0x1F801080..=0x1F8010FC => self.dma.write_u32(addr & 0xFF, data),
            0x1F801100..=0x1F80112F => self.timers.write_u32(addr & 0xFF, data),
            0x1F801810..=0x1F801814 => self.dma_bus.gpu.write_u32(addr & 0xF, data),
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
            0x00000000..=0x00200000 => self.dma_bus.main_ram.read_u16(addr),
            0x80000000..=0x80200000 => self.dma_bus.main_ram.read_u16(addr & 0xFFFFFF),
            0xA0000000..=0xA0200000 => self.dma_bus.main_ram.read_u16(addr & 0xFFFFFF),

            0x1F801070..=0x1F801077 => self.interrupts.read_u16(addr & 0xF),
            0x1F801100..=0x1F80112F => self.timers.read_u16(addr & 0xFF),
            0x1F801C00..=0x1F802000 => self.spu_registers.read_u16((addr & 0xFFF) - 0xC00),
            0xBFC00000..=0xBFC80000 => self.bios.read_u16(addr),
            _ => {
                todo!("u16 write to {:08X}", addr)
            }
        }
    }

    fn write_u16(&mut self, addr: u32, data: u16) {
        assert!(addr % 2 == 0, "unalligned u16 write");

        match addr {
            0x00000000..=0x00200000 => self.dma_bus.main_ram.write_u16(addr, data),
            0x80000000..=0x80200000 => self.dma_bus.main_ram.write_u16(addr & 0xFFFFFF, data),
            0xA0000000..=0xA0200000 => self.dma_bus.main_ram.write_u16(addr & 0xFFFFFF, data),

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
            0x00000000..=0x00200000 => self.dma_bus.main_ram.read_u8(addr),
            0x80000000..=0x80200000 => self.dma_bus.main_ram.read_u8(addr & 0xFFFFFF),
            0xA0000000..=0xA0200000 => self.dma_bus.main_ram.read_u8(addr & 0xFFFFFF),

            0x1F000000..=0x1F080000 => self.expansion_region_1.read_u8(addr & 0xFFFFF),
            0x1F802000..=0x1F802080 => self.expansion_region_2.read_u8(addr & 0xFF),
            0xBFC00000..=0xBFC80000 => self.bios.read_u8(addr),
            _ => {
                todo!("u8 write to {:08X}", addr)
            }
        }
    }

    fn write_u8(&mut self, addr: u32, data: u8) {
        match addr {
            0x00000000..=0x00200000 => self.dma_bus.main_ram.write_u8(addr, data),
            0x80000000..=0x80200000 => self.dma_bus.main_ram.write_u8(addr & 0xFFFFFF, data),
            0xA0000000..=0xA0200000 => self.dma_bus.main_ram.write_u8(addr & 0xFFFFFF, data),

            0x1F000000..=0x1F080000 => self.expansion_region_1.write_u8(addr & 0xFFFFF, data),
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
}
