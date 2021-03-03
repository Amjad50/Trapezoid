use super::BusLine;

const MEMORY_CONTROL_1_DEFAULT_VALUES: &[u32; 9] = &[
    0x1F000000, 0x1F802000, 0x0013243F, 0x00003022, 0x0013243F, 0x200931E1, 0x00020843, 0x00070777,
    0x00031125,
];

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
    fn read_u32(&mut self, addr: u32) -> u32 {
        let addr = addr & 0xFF;
        let index = (addr / 4) as usize;

        self.data[index]
    }

    fn write_u32(&mut self, addr: u32, data: u32) {
        let addr = addr & 0xFF;
        let index = (addr / 4) as usize;

        let should = MEMORY_CONTROL_1_DEFAULT_VALUES[index];

        assert_eq!(
            data, should,
            "mem_ctrl1 wrong value index = {}, should be {:08X}, got {:08X}",
            index, should, data
        );

        self.data[index] = data;
    }
}

// RAM_SIZE
#[derive(Default)]
pub struct MemoryControl2(u32);

impl BusLine for MemoryControl2 {
    fn read_u32(&mut self, _addr: u32) -> u32 {
        self.0
    }

    fn write_u32(&mut self, _addr: u32, data: u32) {
        assert_eq!(
            data, 0xB88,
            "mem_ctrl2 value is wrong, should be 0xB88, got {:08X}",
            data
        );
        self.0 = data;
    }
}

#[derive(Default)]
pub struct CacheControl(u32);

impl BusLine for CacheControl {
    fn read_u32(&mut self, _addr: u32) -> u32 {
        self.0
    }

    fn write_u32(&mut self, _addr: u32, data: u32) {
        // TODO: implement this registerproperly
        println!("LOG cache control written with {:08X}", data);
        self.0 = data;
    }
}
