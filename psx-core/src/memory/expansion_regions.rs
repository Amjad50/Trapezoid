use super::BusLine;

// for now there is no external device to be hooked in PIO extension, so maybe
// use it as ram?
//
// FIXME: check what is the behaviour of expansion regions 1, 2, 3 in the case
//  that there is no device connected.
//
// For now using as ram
pub struct ExpansionRegion1 {
    data: [u8; 0x80000],
}

impl Default for ExpansionRegion1 {
    fn default() -> Self {
        Self { data: [0; 0x80000] }
    }
}

impl BusLine for ExpansionRegion1 {
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
        // POST register used for debugging the BIOS and kernel init
        if addr == 0x41 {
            println!("TraceStep {:02X}", data);
        }

        self.data[addr as usize] = data;
    }
}
