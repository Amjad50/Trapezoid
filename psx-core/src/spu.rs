use byteorder::{ByteOrder, LittleEndian};

use crate::memory::BusLine;

// TODO: properly implement sound registers
pub struct SpuRegisters {
    data: [u8; 0x400],
}

impl Default for SpuRegisters {
    fn default() -> Self {
        Self { data: [0; 0x400] }
    }
}

impl BusLine for SpuRegisters {
    fn read_u32(&mut self, addr: u32) -> u32 {
        let index = addr as usize;

        LittleEndian::read_u32(&self.data[index..index + 4])
    }

    fn write_u32(&mut self, addr: u32, data: u32) {
        let index = addr as usize;

        LittleEndian::write_u32(&mut self.data[index..index + 4], data)
    }

    fn read_u16(&mut self, addr: u32) -> u16 {
        let index = addr as usize;

        LittleEndian::read_u16(&self.data[index..index + 2])
    }

    fn write_u16(&mut self, addr: u32, data: u16) {
        println!("SPU write {:04X} = {}", addr, data);
        let index = addr as usize;

        LittleEndian::write_u16(&mut self.data[index..index + 2], data)
    }
}
