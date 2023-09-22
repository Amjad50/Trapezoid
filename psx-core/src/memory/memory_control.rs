use crate::memory::Result;

use super::BusLine;

#[derive(Default)]
pub struct MemoryControl1 {
    data: [u32; 9],
    // TODO: if these are used, then use them as variables instead of array
    //  using array is just easier
    //expansion_1_base: u32,
    //expansion_2_base: u32,
    //expansion_1_delay_size: u32,
    //expansion_3_delay_size: u32,
    //bios_rom_delay_size: u32,
    //spu_delay_size: u32,
    //cdrom_delay_size: u32,
    //expansion_2_delay_size: u32,
    //common_delay: u32,
}

impl BusLine for MemoryControl1 {
    fn read_u32(&mut self, addr: u32) -> Result<u32> {
        let addr = addr & 0xFF;
        let index = (addr / 4) as usize;

        Ok(self.data[index])
    }

    fn write_u32(&mut self, addr: u32, data: u32) -> Result<()> {
        let addr = addr & 0xFF;
        let index = (addr / 4) as usize;

        log::trace!("mem_ctrl1: index={}, data=0x{:08X}", index, data);

        self.data[index] = data;
        Ok(())
    }
}

// RAM_SIZE
#[derive(Default)]
pub struct MemoryControl2(u32);

impl BusLine for MemoryControl2 {
    fn read_u32(&mut self, _addr: u32) -> Result<u32> {
        Ok(self.0)
    }

    fn write_u32(&mut self, _addr: u32, data: u32) -> Result<()> {
        // TODO: implement different ram modes
        assert!(
            data == 0xB88 || data == 0x888,
            "mem_ctrl2 value is wrong, should be 0xB88, got {:08X}",
            data
        );
        self.0 = data;
        Ok(())
    }
}

#[derive(Default)]
pub struct CacheControl(u32);

impl BusLine for CacheControl {
    fn read_u32(&mut self, _addr: u32) -> Result<u32> {
        Ok(self.0)
    }

    fn write_u32(&mut self, _addr: u32, data: u32) -> Result<()> {
        // TODO: implement this registerproperly
        log::info!("LOG cache control written with {:08X}", data);
        self.0 = data;
        Ok(())
    }
}
