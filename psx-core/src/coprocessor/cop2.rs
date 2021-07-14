bitflags::bitflags! {
    #[derive(Default)]
    struct Flag: u32 {
        const IR0_SATURATED_TO_P0000_P1000                = 0b00000000000000000001000000000000;
        const SY2_SATURATED_TO_N0400_P03FF                = 0b00000000000000000010000000000000;
        const SX2_SATURATED_TO_N0400_P03FF                = 0b00000000000000000100000000000000;
        const MAC0_RES_LARGER_THAN_31_BITS_NEG            = 0b00000000000000001000000000000000;
        const MAC0_RES_LARGER_THAN_31_BITS_POS            = 0b00000000000000010000000000000000;
        const DIVIDE_OVERFLOW                             = 0b00000000000000100000000000000000;
        const SZ3_OR_OTZ_SATURATED_TO_0000_FFFF           = 0b00000000000001000000000000000000;
        const COLOR_FIFO_B_SATURATED_TO_00_FF             = 0b00000000000010000000000000000000;
        const COLOR_FIFO_G_SATURATED_TO_00_FF             = 0b00000000000100000000000000000000;
        const COLOR_FIFO_R_SATURATED_TO_00_FF             = 0b00000000001000000000000000000000;
        const IR3_SATURATED_TO_P0000_P7FFF_OR_N8000_P7FFF = 0b00000000010000000000000000000000;
        const IR2_SATURATED_TO_P0000_P7FFF_OR_N8000_P7FFF = 0b00000000100000000000000000000000;
        const IR1_SATURATED_TO_P0000_P7FFF_OR_N8000_P7FFF = 0b00000001000000000000000000000000;
        const MAC3_RES_LARGER_THAN_43_BITS_NEG            = 0b00000010000000000000000000000000;
        const MAC2_RES_LARGER_THAN_43_BITS_NEG            = 0b00000100000000000000000000000000;
        const MAC1_RES_LARGER_THAN_43_BITS_NEG            = 0b00001000000000000000000000000000;
        const MAC3_RES_LARGER_THAN_43_BITS_POS            = 0b00010000000000000000000000000000;
        const MAC2_RES_LARGER_THAN_43_BITS_POS            = 0b00100000000000000000000000000000;
        const MAC1_RES_LARGER_THAN_43_BITS_POS            = 0b01000000000000000000000000000000;
        // const NOT_USED                                 = 0b10000000000000000000111111111111;
    }
}

impl Flag {
    fn bits_with_error(&self) -> u32 {
        // error bit is set if any of the bits (30-22, 18-13)
        let error = (self.bits & 0b01111111100001111110000000000000 != 0) as u32;

        self.bits | error << 31
    }
}

#[derive(Default)]
struct Vector<T> {
    pub x: T,
    pub y: T,
    pub z: T,
}

#[derive(Default)]
pub struct Gte {
    vectors: [Vector<i16>; 3],
    rgbc: u32,
    otz: u16,
    ir: [i16; 4],
    res1: u32,
    mac: [i32; 4],
    sxy: [(i16, i16); 3],
    sz: [u16; 4],
    rgb: [u32; 3],
    irgb: u16,
    orgb: u16,
    lzcs: i32,
    lzcr: u32,

    rotation_matrix: [[i16; 3]; 3],
    translation_vector: [i32; 3],
    light_source_matrix: [[i16; 3]; 3],
    light_color_matrix: [[i16; 3]; 3],

    background_color: [u32; 3],
    far_color: [u32; 3],
    screen_offset: (u32, u32),
    projection_plain_distance: u16,
    dqa: i16,
    dqb: u32,
    zsf3: i16,
    zsf4: i16,
    flag: Flag,
}

impl Gte {
    /// updates the ir 1, 2, 3 registers on any write/change to irgb
    fn update_ir123(&mut self) {
        let r = (self.irgb >> 0) & 0x1F;
        let g = (self.irgb >> 5) & 0x1F;
        let b = (self.irgb >> 10) & 0x1F;

        self.ir[1] = (r * 0x80) as i16;
        self.ir[2] = (g * 0x80) as i16;
        self.ir[3] = (b * 0x80) as i16;
    }

    /// updates orgb register on any write/change to ir 1,2,3
    /// orgb also acts as irgb mirror
    fn update_orgb(&mut self) {
        let r = (self.ir[1] / 80).max(0).min(0x1F) as u16;
        let g = (self.ir[2] / 80).max(0).min(0x1F) as u16;
        let b = (self.ir[3] / 80).max(0).min(0x1F) as u16;

        self.orgb = b << 10 | g << 5 | r;
    }

    /// count the number of leading ones or zeros from lzcs
    fn update_lzcr(&mut self) {
        if self.lzcs.is_negative() {
            self.lzcr = self.lzcs.leading_ones()
        } else {
            self.lzcr = self.lzcs.leading_zeros()
        }
    }
}

impl Gte {
    pub fn read_data(&self, num: u8) -> u32 {
        assert!(num <= 0x1F);

        let out = match num {
            0 | 2 | 4 => {
                ((self.vectors[num as usize / 2].y as u16 as u32) << 16)
                    | self.vectors[num as usize / 2].x as u16 as u32
            }
            1 | 3 | 5 => self.vectors[num as usize / 2].z as i32 as u32,
            6 => self.rgbc,
            7 => self.otz as u32,
            8..=11 => self.ir[num as usize - 8] as i32 as u32,
            12..=14 => {
                // (x, y)
                ((self.sxy[num as usize - 12].1 as u16 as u32) << 16)
                    | self.sxy[num as usize - 12].0 as u16 as u32
            }
            15 => ((self.sxy[2].1 as u16 as u32) << 16) | self.sxy[2].0 as u16 as u32,
            16..=19 => self.sz[num as usize - 16] as u32,
            20..=22 => self.rgb[num as usize - 20],
            23 => self.res1,
            24..=27 => self.mac[num as usize - 24] as u32,
            28 => self.irgb as u32,
            29 => self.orgb as u32,
            30 => self.lzcs as u32,
            31 => self.lzcr,
            _ => unreachable!(),
        };

        log::info!("cop2 data read {}, data={:08X}", num, out);
        out
    }

    pub fn write_data(&mut self, num: u8, data: u32) {
        assert!(num <= 0x1F);
        log::info!("cop2 data write {}, data={:08X}", num, data);

        let lsb = (data & 0xFFFF) as i16;
        let msb = ((data >> 16) & 0xFFFF) as i16;

        match num {
            0 | 2 | 4 => {
                self.vectors[num as usize / 2].x = lsb;
                self.vectors[num as usize / 2].y = msb;
            }
            1 | 3 | 5 => self.vectors[num as usize / 2].z = (data & 0xFFFF) as i16,
            6 => self.rgbc = data,
            7 => self.otz = data as u16,
            8..=11 => {
                self.ir[num as usize - 8] = (data & 0xFFFF) as i16;
                self.update_orgb();
            }
            12..=14 => {
                // (x, y)
                self.sxy[num as usize - 12] = (lsb, msb);
            }
            15 => {
                // move on write
                self.sxy[0] = self.sxy[1];
                self.sxy[1] = self.sxy[2];
                self.sxy[2] = (lsb, msb);
            }
            16..=19 => self.sz[num as usize - 16] = data as u16,
            20..=22 => self.rgb[num as usize - 20] = data,
            23 => self.res1 = data,
            24..=27 => self.mac[num as usize - 24] = data as i32,
            28 => {
                self.irgb = (data as u16) & 0x7FFF;
                self.orgb = self.irgb;
                self.update_ir123();
            }
            29 => {} // orgb is read only
            30 => {
                self.lzcs = data as i32;
                self.update_lzcr();
            }
            31 => {} // lzcr is read only
            _ => unreachable!(),
        }
    }

    pub fn read_ctrl(&self, num: u8) -> u32 {
        assert!(num <= 0x1F);

        let out = match num {
            0 => {
                ((self.rotation_matrix[0][1] as u16 as u32) << 16)
                    | self.rotation_matrix[0][0] as u16 as u32
            }
            1 => {
                ((self.rotation_matrix[1][0] as u16 as u32) << 16)
                    | self.rotation_matrix[0][2] as u16 as u32
            }
            2 => {
                ((self.rotation_matrix[1][2] as u16 as u32) << 16)
                    | self.rotation_matrix[1][1] as u16 as u32
            }
            3 => {
                ((self.rotation_matrix[2][1] as u16 as u32) << 16)
                    | self.rotation_matrix[2][0] as u16 as u32
            }
            4 => self.rotation_matrix[2][2] as i32 as u32,
            5..=7 => self.translation_vector[num as usize - 5] as u32,
            8 => {
                ((self.light_source_matrix[0][1] as u16 as u32) << 16)
                    | self.light_source_matrix[0][0] as u16 as u32
            }
            9 => {
                ((self.light_source_matrix[1][0] as u16 as u32) << 16)
                    | self.light_source_matrix[0][2] as u16 as u32
            }
            10 => {
                ((self.light_source_matrix[1][2] as u16 as u32) << 16)
                    | self.light_source_matrix[1][1] as u16 as u32
            }
            11 => {
                ((self.light_source_matrix[2][1] as u16 as u32) << 16)
                    | self.light_source_matrix[2][0] as u16 as u32
            }
            12 => self.light_source_matrix[2][2] as i32 as u32,
            13..=15 => self.background_color[num as usize - 13],
            16 => {
                ((self.light_color_matrix[0][1] as u16 as u32) << 16)
                    | self.light_color_matrix[0][0] as u16 as u32
            }
            17 => {
                ((self.light_color_matrix[1][0] as u16 as u32) << 16)
                    | self.light_color_matrix[0][2] as u16 as u32
            }
            18 => {
                ((self.light_color_matrix[1][2] as u16 as u32) << 16)
                    | self.light_color_matrix[1][1] as u16 as u32
            }
            19 => {
                ((self.light_color_matrix[2][1] as u16 as u32) << 16)
                    | self.light_color_matrix[2][0] as u16 as u32
            }
            20 => self.light_color_matrix[2][2] as i32 as u32,
            21..=23 => self.far_color[num as usize - 21],
            24 => self.screen_offset.0,
            25 => self.screen_offset.1,
            26 => self.projection_plain_distance as i16 as i32 as u32, // bug sign extend on read only
            27 => self.dqa as u32,
            28 => self.dqb,
            29 => self.zsf3 as u32,
            30 => self.zsf4 as u32,
            31 => self.flag.bits_with_error(),
            _ => unreachable!(),
        };

        log::info!("cop2 ctrl read {}, data={:08X}", num, out);
        out
    }

    pub fn write_ctrl(&mut self, num: u8, data: u32) {
        assert!(num <= 0x1F);
        log::info!("cop2 ctrl write {}, data={:08X}", num, data);

        let lsb = (data & 0xFFFF) as i16;
        let msb = ((data >> 16) & 0xFFFF) as i16;

        match num {
            0 => {
                self.rotation_matrix[0][0] = lsb;
                self.rotation_matrix[0][1] = msb;
            }
            1 => {
                self.rotation_matrix[0][2] = lsb;
                self.rotation_matrix[1][0] = msb;
            }
            2 => {
                self.rotation_matrix[1][1] = lsb;
                self.rotation_matrix[1][2] = msb;
            }
            3 => {
                self.rotation_matrix[2][0] = lsb;
                self.rotation_matrix[2][1] = msb;
            }
            4 => self.rotation_matrix[2][2] = lsb,
            5..=7 => self.translation_vector[num as usize - 5] = data as i32,
            8 => {
                self.light_source_matrix[0][0] = lsb;
                self.light_source_matrix[0][1] = msb;
            }
            9 => {
                self.light_source_matrix[0][2] = lsb;
                self.light_source_matrix[1][0] = msb;
            }
            10 => {
                self.light_source_matrix[1][1] = lsb;
                self.light_source_matrix[1][2] = msb;
            }
            11 => {
                self.light_source_matrix[2][0] = lsb;
                self.light_source_matrix[2][1] = msb;
            }
            12 => self.light_source_matrix[2][2] = lsb,
            13..=15 => self.background_color[num as usize - 13] = data,
            16 => {
                self.light_color_matrix[0][0] = lsb;
                self.light_color_matrix[0][1] = msb;
            }
            17 => {
                self.light_color_matrix[0][2] = lsb;
                self.light_color_matrix[1][0] = msb;
            }
            18 => {
                self.light_color_matrix[1][1] = lsb;
                self.light_color_matrix[1][2] = msb;
            }
            19 => {
                self.light_color_matrix[2][0] = lsb;
                self.light_color_matrix[2][1] = msb;
            }
            20 => self.light_color_matrix[2][2] = lsb,
            21..=23 => self.far_color[num as usize - 21] = data,
            24 => self.screen_offset.0 = data,
            25 => self.screen_offset.1 = data,
            26 => self.projection_plain_distance = data as u16,
            27 => self.dqa = (data & 0xFFFF) as i16,
            28 => self.dqb = data,
            29 => self.zsf3 = data as i16,
            30 => self.zsf4 = data as i16,
            31 => self.flag = Flag::from_bits_truncate(data),
            _ => unreachable!(),
        }
    }
}
