use byteorder::{ByteOrder, LittleEndian};

use super::BusLine;

pub struct MainRam {
    data: Vec<u8>,
}

impl Default for MainRam {
    fn default() -> Self {
        Self {
            data: vec![0; 0x200000],
        }
    }
}

impl BusLine for MainRam {
    fn read_u32(&mut self, addr: u32) -> u32 {
        let index = addr as usize;

        LittleEndian::read_u32(&self.data[index..index + 4])
    }

    fn write_u32(&mut self, addr: u32, data: u32) {
        let index = addr as usize;

        LittleEndian::write_u32(&mut self.data[index..index + 4], data)
    }
}
