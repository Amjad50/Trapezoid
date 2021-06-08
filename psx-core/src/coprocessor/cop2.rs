#[derive(Default)]
pub struct Gte {
    background_color: (u32, u32, u32),
    far_color: (u32, u32, u32),
    screen_offset: (u32, u32),
    projection_plain_distance: u32,
    dqa: u32,
    dqb: u32,
    zsf3: u32,
    zsf4: u32,
}

impl Gte {
    pub fn read_ctrl(&self, num: u8) -> u32 {
        assert!(num <= 0x1F);
        todo!("cop2 ctrl read {}", num)
    }

    pub fn write_ctrl(&mut self, num: u8, data: u32) {
        assert!(num <= 0x1F);
        log::info!("cop2 ctrl write {}, data={:08X}", num, data);
        match num {
            13 => self.background_color.0 = data,
            14 => self.background_color.1 = data,
            15 => self.background_color.2 = data,
            21 => self.far_color.0 = data,
            22 => self.far_color.1 = data,
            23 => self.far_color.2 = data,
            24 => self.screen_offset.0 = data,
            25 => self.screen_offset.1 = data,
            26 => self.projection_plain_distance = data,
            27 => self.dqa = data,
            28 => self.dqb = data,
            29 => self.zsf3 = data,
            30 => self.zsf4 = data,
            _ => todo!("cop2 ctrl write {}, data={:08X}", num, data),
        }
    }

    pub fn read_data(&self, num: u8) -> u32 {
        assert!(num <= 0x1F);
        todo!("cop2 data read {}", num)
    }

    pub fn write_data(&mut self, num: u8, data: u32) {
        assert!(num <= 0x1F);
        todo!("cop2 data write {}, data={:08X}", num, data)
    }
}
