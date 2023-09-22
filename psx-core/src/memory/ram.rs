use byteorder::{ByteOrder, LittleEndian};

use crate::memory::Result;

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

impl MainRam {
    pub fn put_at_address(&mut self, block_data: &[u8], addr: u32) {
        let addr = (addr as usize) & 0x1FFFFF;
        let block_len = block_data.len();
        assert!((block_len + addr) < self.data.len());

        self.data[addr..(addr + block_len)].copy_from_slice(block_data);
    }
}

impl BusLine for MainRam {
    fn read_u32(&mut self, addr: u32) -> Result<u32> {
        let index = (addr as usize) & 0x1FFFFF;

        Ok(LittleEndian::read_u32(&self.data[index..index + 4]))
    }

    fn write_u32(&mut self, addr: u32, data: u32) -> Result<()> {
        let index = (addr as usize) & 0x1FFFFF;

        LittleEndian::write_u32(&mut self.data[index..index + 4], data);
        Ok(())
    }

    fn read_u16(&mut self, addr: u32) -> Result<u16> {
        let index = (addr as usize) & 0x1FFFFF;
        Ok(LittleEndian::read_u16(&self.data[index..index + 2]))
    }

    fn write_u16(&mut self, addr: u32, data: u16) -> Result<()> {
        let index = (addr as usize) & 0x1FFFFF;

        LittleEndian::write_u16(&mut self.data[index..index + 2], data);
        Ok(())
    }

    fn read_u8(&mut self, addr: u32) -> Result<u8> {
        Ok(self.data[(addr as usize) & 0x1FFFFF])
    }

    fn write_u8(&mut self, addr: u32, data: u8) -> Result<()> {
        self.data[(addr as usize) & 0x1FFFFF] = data;
        Ok(())
    }
}

pub struct Scratchpad {
    data: Vec<u8>,
}

impl Default for Scratchpad {
    fn default() -> Self {
        Self {
            data: vec![0; 0x400],
        }
    }
}

impl BusLine for Scratchpad {
    fn read_u32(&mut self, addr: u32) -> Result<u32> {
        let index = addr as usize;

        Ok(LittleEndian::read_u32(&self.data[index..index + 4]))
    }

    fn write_u32(&mut self, addr: u32, data: u32) -> Result<()> {
        let index = addr as usize;

        LittleEndian::write_u32(&mut self.data[index..index + 4], data);
        Ok(())
    }

    fn read_u16(&mut self, addr: u32) -> Result<u16> {
        let index = addr as usize;

        Ok(LittleEndian::read_u16(&self.data[index..index + 2]))
    }

    fn write_u16(&mut self, addr: u32, data: u16) -> Result<()> {
        let index = addr as usize;

        LittleEndian::write_u16(&mut self.data[index..index + 2], data);
        Ok(())
    }

    fn read_u8(&mut self, addr: u32) -> Result<u8> {
        Ok(self.data[addr as usize])
    }

    fn write_u8(&mut self, addr: u32, data: u8) -> Result<()> {
        self.data[addr as usize] = data;
        Ok(())
    }
}
