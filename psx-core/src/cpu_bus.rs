use crate::cpu::CpuBusProvider;

pub struct Bios {
    data: Vec<u8>,
}

impl Bios {
    // TODO: produce a valid `Error` struct
    pub fn from_file<P: AsRef<Path>>(bios_file_path: P) -> Result<Self, ()> {}
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
        todo!()
    }

    fn write(&mut self, addr: u32, data: u8) {
        todo!()
    }
}
