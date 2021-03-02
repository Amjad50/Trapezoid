mod memory_control;

use std::fs::File;
use std::io::Read;
use std::path::Path;

use byteorder::{ByteOrder, LittleEndian};

use memory_control::{CacheControl, MemoryControl1, MemoryControl2};

pub trait BusLine {
    fn read_u32(&mut self, addr: u32) -> u32;
    fn write_u32(&mut self, addr: u32, data: u32);
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
}

pub struct CpuBus {
    bios: Bios,
    mem_ctrl_1: MemoryControl1,
    mem_ctrl_2: MemoryControl2,
    cache_control: CacheControl,
}

impl CpuBus {
    pub fn new(bios: Bios) -> Self {
        Self {
            bios,
            mem_ctrl_1: MemoryControl1::default(),
            mem_ctrl_2: MemoryControl2::default(),
            cache_control: CacheControl::default(),
        }
    }
}

impl BusLine for CpuBus {
    fn read_u32(&mut self, addr: u32) -> u32 {
        assert!(addr % 4 == 0, "unalligned read");

        match addr {
            0xBFC00000..=0xBFC80000 => self.bios.read_u32(addr),
            0x1F801000..=0x1F801020 => self.mem_ctrl_1.read_u32(addr),
            0x1F801060 => self.mem_ctrl_2.read_u32(addr),
            0xFFFE0130 => self.cache_control.read_u32(addr),
            _ => {
                todo!("read from {:08X}", addr)
            }
        }
    }

    fn write_u32(&mut self, addr: u32, data: u32) {
        assert!(addr % 4 == 0, "unalligned write");

        match addr {
            0x1F801000..=0x1F801020 => self.mem_ctrl_1.write_u32(addr, data),
            0x1F801060 => self.mem_ctrl_2.write_u32(addr, data),
            0xFFFE0130 => self.cache_control.write_u32(addr, data),
            _ => {
                todo!("write to {:08X}", addr)
            }
        }
    }
}
