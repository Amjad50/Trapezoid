#[derive(Debug)]
enum GteCommandOpcode {
    Na,

    Rtps,
    Rtpt,

    Mvmva,

    Dcpl,
    Dpcs,
    Dpct,
    Intpl,
    Sqr,

    Ncs,
    Nct,
    Ncds,
    Ncdt,
    Nccs,
    Ncct,
    Cdp,
    Cc,
    Nclip,
    Avsz3,
    Avsz4,
    Op,

    Gpf,
    Gpl,
}

impl GteCommandOpcode {
    fn from_real_cmd(real_cmd: u8) -> Self {
        match real_cmd {
            0x01 => Self::Rtps,
            0x06 => Self::Nclip,
            0x0C => Self::Op,
            0x10 => Self::Dpcs,
            0x11 => Self::Intpl,
            0x12 => Self::Mvmva,
            0x13 => Self::Ncds,
            0x14 => Self::Cdp,
            0x16 => Self::Ncdt,
            0x1B => Self::Nccs,
            0x1C => Self::Cc,
            0x1E => Self::Ncs,
            0x20 => Self::Nct,
            0x28 => Self::Sqr,
            0x29 => Self::Dcpl,
            0x2A => Self::Dpct,
            0x2D => Self::Avsz3,
            0x2E => Self::Avsz4,
            0x30 => Self::Rtpt,
            0x3D => Self::Gpf,
            0x3E => Self::Gpl,
            0x3F => Self::Ncct,
            _ => Self::Na,
        }
    }
}

#[derive(Debug)]
struct GteCommand {
    opcode: GteCommandOpcode,
    lm: bool,
    sf: bool,
    tx: u8,
    vx: u8,
    mx: u8,
}

impl GteCommand {
    fn from_u32(cmd: u32) -> Self {
        // 20-24  Fake GTE Command Number (00h..1Fh) (ignored by hardware)
        // 19     sf - Shift Fraction in IR registers (0=No fraction, 1=12bit fraction)
        // 17-18  MVMVA Multiply Matrix    (0=Rotation. 1=Light, 2=Color, 3=Reserved)
        // 15-16  MVMVA Multiply Vector    (0=V0, 1=V1, 2=V2, 3=IR/long)
        // 13-14  MVMVA Translation Vector (0=TR, 1=BK, 2=FC/Bugged, 3=None)
        // 10     lm - Saturate IR1,IR2,IR3 result (0=To -8000h..+7FFFh, 1=To 0..+7FFFh)
        // 0-5    Real GTE Command Number (00h..3Fh) (used by hardware)

        let real_cmd = cmd & 0x3F;
        let lm = (cmd >> 10) & 1 != 0;
        let sf = (cmd >> 19) & 1 != 0;
        let translation_vector = ((cmd >> 13) & 3) as u8;
        let multiply_vector = ((cmd >> 15) & 3) as u8;
        let multiply_matrix = ((cmd >> 17) & 3) as u8;

        Self {
            opcode: GteCommandOpcode::from_real_cmd(real_cmd as u8),
            lm,
            sf,
            tx: translation_vector,
            vx: multiply_vector,
            mx: multiply_matrix,
        }
    }
}

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
pub struct Gte {
    vectors: [[i16; 3]; 3],
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
    screen_offset: [i32; 2],
    projection_plain_distance: u16,
    dqa: i16,
    dqb: i32,
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
    fn update_orgb_irgb(&mut self) {
        let r = (self.ir[1] / 80).max(0).min(0x1F) as u16;
        let g = (self.ir[2] / 80).max(0).min(0x1F) as u16;
        let b = (self.ir[3] / 80).max(0).min(0x1F) as u16;

        self.orgb = b << 10 | g << 5 | r;
        self.irgb = self.orgb;
    }

    /// count the number of leading ones or zeros from lzcs
    fn update_lzcr(&mut self) {
        if self.lzcs.is_negative() {
            self.lzcr = self.lzcs.leading_ones()
        } else {
            self.lzcr = self.lzcs.leading_zeros()
        }
    }

    fn push_sxy_fifo(&mut self, x: i16, y: i16) {
        self.sxy[0] = self.sxy[1];
        self.sxy[1] = self.sxy[2];
        self.sxy[2] = (x, y);
    }
}

impl Gte {
    pub fn read_data(&self, num: u8) -> u32 {
        assert!(num <= 0x1F);

        let out = match num {
            0 | 2 | 4 => {
                ((self.vectors[num as usize / 2][1] as u16 as u32) << 16)
                    | self.vectors[num as usize / 2][0] as u16 as u32
            }
            1 | 3 | 5 => self.vectors[num as usize / 2][2] as i32 as u32,
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
                self.vectors[num as usize / 2][0] = lsb;
                self.vectors[num as usize / 2][1] = msb;
            }
            1 | 3 | 5 => self.vectors[num as usize / 2][2] = (data & 0xFFFF) as i16,
            6 => self.rgbc = data,
            7 => self.otz = data as u16,
            8..=11 => {
                self.ir[num as usize - 8] = (data & 0xFFFF) as i16;
                self.update_orgb_irgb();
            }
            12..=14 => {
                // (x, y)
                self.sxy[num as usize - 12] = (lsb, msb);
            }
            15 => {
                // move on write
                self.push_sxy_fifo(lsb, msb);
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
            24 => self.screen_offset[0] as u32,
            25 => self.screen_offset[1] as u32,
            26 => self.projection_plain_distance as i16 as i32 as u32, // bug sign extend on read only
            27 => self.dqa as u32,
            28 => self.dqb as u32,
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
            24 => self.screen_offset[0] = data as i32,
            25 => self.screen_offset[1] = data as i32,
            26 => self.projection_plain_distance = data as u16,
            27 => self.dqa = (data & 0xFFFF) as i16,
            28 => self.dqb = data as i32,
            29 => self.zsf3 = data as i16,
            30 => self.zsf4 = data as i16,
            31 => self.flag = Flag::from_bits_truncate(data),
            _ => unreachable!(),
        }
    }

    pub fn execute_command(&mut self, cmd: u32) {
        let cmd = GteCommand::from_u32(cmd);

        log::info!("cop2 executing command {:?}", cmd);

        match cmd.opcode {
            // GteCommandOpcode::Na => todo!(),
            // GteCommandOpcode::Rtps => todo!(),
            // GteCommandOpcode::Rtpt => todo!(),
            // GteCommandOpcode::Mvmva => todo!(),
            // GteCommandOpcode::Dcpl => todo!(),
            // GteCommandOpcode::Dpcs => todo!(),
            // GteCommandOpcode::Dpct => todo!(),
            // GteCommandOpcode::Intpl => todo!(),
            // GteCommandOpcode::Sqr => todo!(),
            // GteCommandOpcode::Ncs => todo!(),
            // GteCommandOpcode::Nct => todo!(),
            // GteCommandOpcode::Ncds => todo!(),
            // GteCommandOpcode::Ncdt => todo!(),
            // GteCommandOpcode::Nccs => todo!(),
            // GteCommandOpcode::Ncct => todo!(),
            // GteCommandOpcode::Cdp => todo!(),
            // GteCommandOpcode::Cc => todo!(),
            // GteCommandOpcode::Nclip => todo!(),
            // GteCommandOpcode::Avsz3 => todo!(),
            // GteCommandOpcode::Avsz4 => todo!(),
            // GteCommandOpcode::Op => todo!(),
            // GteCommandOpcode::Gpf => todo!(),
            // GteCommandOpcode::Gpl => todo!(),
            _ => todo!("cop2 unimplemented_command {:?}", cmd.opcode),
        }
    }
}
