mod expansion_regions;
mod memory_control;
mod ram;

use std::fs::File;
use std::io::Read;
use std::path::Path;

use byteorder::{ByteOrder, LittleEndian};

use crate::spu::SpuRegisters;
use expansion_regions::{ExpansionRegion1, ExpansionRegion2};
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

pub struct CpuBus {
    bios: Bios,
    mem_ctrl_1: MemoryControl1,
    mem_ctrl_2: MemoryControl2,
    cache_control: CacheControl,

    main_ram: MainRam,

    spu_registers: SpuRegisters,

    expansion_region_1: ExpansionRegion1,
    expansion_region_2: ExpansionRegion2,
}

impl CpuBus {
    pub fn new(bios: Bios) -> Self {
        Self {
            bios,
            mem_ctrl_1: MemoryControl1::default(),
            mem_ctrl_2: MemoryControl2::default(),
            cache_control: CacheControl::default(),
            main_ram: MainRam::default(),
            spu_registers: SpuRegisters::default(),
            expansion_region_1: ExpansionRegion1::default(),
            expansion_region_2: ExpansionRegion2::default(),
        }
    }
}

impl BusLine for CpuBus {
    fn read_u32(&mut self, addr: u32) -> u32 {
        assert!(addr % 4 == 0, "unalligned u32 read");

        match addr {
            0x00000000..=0x00200000 => self.main_ram.read_u32(addr),
            // TODO: implement mirroring in a better way (with cache as well maybe)
            0x80000000..=0x80200000 => self.main_ram.read_u32(addr & 0xFFFFFF),
            0xA0000000..=0xA0200000 => self.main_ram.read_u32(addr & 0xFFFFFF),
            0xBFC00000..=0xBFC80000 => self.bios.read_u32(addr),
            0x1F801000..=0x1F801020 => self.mem_ctrl_1.read_u32(addr),
            0x1F801060 => self.mem_ctrl_2.read_u32(addr),
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
            0x00000000..=0x00200000 => self.main_ram.write_u32(addr, data),
            0x80000000..=0x80200000 => self.main_ram.write_u32(addr & 0xFFFFFF, data),
            0xA0000000..=0xA0200000 => self.main_ram.write_u32(addr & 0xFFFFFF, data),
            0x1F801000..=0x1F801020 => self.mem_ctrl_1.write_u32(addr, data),
            0x1F801060 => self.mem_ctrl_2.write_u32(addr, data),
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
            0x00000000..=0x00200000 => self.main_ram.read_u16(addr),
            0x80000000..=0x80200000 => self.main_ram.read_u16(addr & 0xFFFFFF),
            0xA0000000..=0xA0200000 => self.main_ram.read_u16(addr & 0xFFFFFF),

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
            0x00000000..=0x00200000 => self.main_ram.write_u16(addr, data),
            0x80000000..=0x80200000 => self.main_ram.write_u16(addr & 0xFFFFFF, data),
            0xA0000000..=0xA0200000 => self.main_ram.write_u16(addr & 0xFFFFFF, data),

            0x1F801C00..=0x1F802000 => self.spu_registers.write_u16((addr & 0xFFF) - 0xC00, data),
            _ => {
                todo!("u16 write to {:08X}", addr)
            }
        }
    }
    fn read_u8(&mut self, addr: u32) -> u8 {
        match addr {
            0x00000000..=0x00200000 => self.main_ram.read_u8(addr),
            0x80000000..=0x80200000 => self.main_ram.read_u8(addr & 0xFFFFFF),
            0xA0000000..=0xA0200000 => self.main_ram.read_u8(addr & 0xFFFFFF),

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
            0x00000000..=0x00200000 => self.main_ram.write_u8(addr, data),
            0x80000000..=0x80200000 => self.main_ram.write_u8(addr & 0xFFFFFF, data),
            0xA0000000..=0xA0200000 => self.main_ram.write_u8(addr & 0xFFFFFF, data),

            0x1F000000..=0x1F080000 => self.expansion_region_1.write_u8(addr & 0xFFFFF, data),
            0x1F802000..=0x1F802080 => self.expansion_region_2.write_u8(addr & 0xFF, data),
            _ => {
                todo!("u8 write to {:08X}", addr)
            }
        }
    }
}
