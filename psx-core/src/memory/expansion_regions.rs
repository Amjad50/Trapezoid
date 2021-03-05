use super::BusLine;

pub struct ExpansionRegion2 {
    data: [u8; 0x80],
}

impl Default for ExpansionRegion2 {
    fn default() -> Self {
        Self { data: [0; 0x80] }
    }
}

impl BusLine for ExpansionRegion2 {
    fn read_u32(&mut self, _addr: u32) -> u32 {
        todo!()
    }

    fn write_u32(&mut self, _addr: u32, _data: u32) {
        todo!()
    }

    fn read_u16(&mut self, _addr: u32) -> u16 {
        todo!()
    }

    fn write_u16(&mut self, _addr: u32, _data: u16) {
        todo!()
    }

    fn read_u8(&mut self, addr: u32) -> u8 {
        self.data[addr as usize]
    }

    fn write_u8(&mut self, addr: u32, data: u8) {
        self.data[addr as usize] = data;
    }
}
