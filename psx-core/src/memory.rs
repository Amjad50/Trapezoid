use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::cpu::CpuBusProvider;

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

    pub fn read(&self, addr: u32) -> u8 {
        let index = (addr & 0xFFFFF) as usize;

        self.data[index]
    }
}

pub struct CpuBus {
    bios: Bios,
}

impl CpuBus {
    pub fn new(bios: Bios) -> Self {
        Self { bios }
    }
}

impl CpuBusProvider for CpuBus {
    fn read(&mut self, addr: u32) -> u8 {
        match addr {
            0xBFC00000..=0xBFC80000 => self.bios.read(addr),
            _ => {
                todo!()
            }
        }
    }

    fn write(&mut self, addr: u32, data: u8) {
        todo!()
    }
}
