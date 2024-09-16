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
    #[derive(Default, Clone, Copy)]
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
        let error = (self.bits() & 0b01111111100001111110000000000000 != 0) as u32;

        self.bits() | error << 31
    }
}

#[inline(always)]
fn get_rgb(rgb: u32) -> (i64, i64, i64) {
    let r = (rgb & 0xFF) as i64;
    let g = ((rgb >> 8) & 0xFF) as i64;
    let b = ((rgb >> 16) & 0xFF) as i64;
    (r, g, b)
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

    background_color: [i32; 3],
    far_color: [i32; 3],
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
        let r = (self.irgb) & 0x1F;
        let g = (self.irgb >> 5) & 0x1F;
        let b = (self.irgb >> 10) & 0x1F;

        self.ir[1] = (r * 0x80) as i16;
        self.ir[2] = (g * 0x80) as i16;
        self.ir[3] = (b * 0x80) as i16;
    }

    /// updates orgb register on any write/change to ir 1,2,3
    /// orgb also acts as irgb mirror
    fn update_orgb_irgb(&mut self) {
        let r = (self.ir[1] >> 7).clamp(0, 0x1F) as u16;
        let g = (self.ir[2] >> 7).clamp(0, 0x1F) as u16;
        let b = (self.ir[3] >> 7).clamp(0, 0x1F) as u16;

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

    fn push_sz_fifo(&mut self, z: u16) {
        self.sz[0] = self.sz[1];
        self.sz[1] = self.sz[2];
        self.sz[2] = self.sz[3];
        self.sz[3] = z;
    }

    fn push_sxy_fifo(&mut self, x: i16, y: i16) {
        self.sxy[0] = self.sxy[1];
        self.sxy[1] = self.sxy[2];
        self.sxy[2] = (x, y);
    }

    fn push_color_fifo(&mut self, r: i32, g: i32, b: i32, code: u8) {
        self.rgb[0] = self.rgb[1];
        self.rgb[1] = self.rgb[2];
        let r = self.saturate_put_flag(r, 0x00, 0xFF, Flag::COLOR_FIFO_R_SATURATED_TO_00_FF) as u32;
        let g = self.saturate_put_flag(g, 0x00, 0xFF, Flag::COLOR_FIFO_G_SATURATED_TO_00_FF) as u32;
        let b = self.saturate_put_flag(b, 0x00, 0xFF, Flag::COLOR_FIFO_B_SATURATED_TO_00_FF) as u32;

        self.rgb[2] = (code as u32) << 24 | b << 16 | g << 8 | r;
    }
}

impl Gte {
    fn saturate_put_flag<T: Ord>(&mut self, value: T, min: T, max: T, flag: Flag) -> T {
        if value < min {
            self.flag.insert(flag);
            min
        } else if value > max {
            self.flag.insert(flag);
            max
        } else {
            value
        }
    }

    fn set_ir0(&mut self, value: i32) {
        self.ir[0] =
            self.saturate_put_flag(value, 0x0000, 0x1000, Flag::IR0_SATURATED_TO_P0000_P1000)
                as i16;
    }

    fn set_mac0(&mut self, mac0: i64) {
        if mac0 < -(1 << 31) {
            self.flag.insert(Flag::MAC0_RES_LARGER_THAN_31_BITS_NEG);
        } else if mac0 > (1 << 31) - 1 {
            self.flag.insert(Flag::MAC0_RES_LARGER_THAN_31_BITS_POS);
        }

        self.mac[0] = mac0 as i32;
    }

    fn copy_mac_ir_saturate(&mut self, lm: bool) {
        let min = if lm { 0 } else { -0x8000 };
        let flags = &[
            Flag::IR1_SATURATED_TO_P0000_P7FFF_OR_N8000_P7FFF,
            Flag::IR2_SATURATED_TO_P0000_P7FFF_OR_N8000_P7FFF,
            Flag::IR3_SATURATED_TO_P0000_P7FFF_OR_N8000_P7FFF,
        ];

        for i in 1..=3 {
            self.ir[i] = self.saturate_put_flag(self.mac[i], min, 0x7FFF, flags[i - 1]) as i16;
        }

        // because of a change in ir1,2,3 vector
        self.update_orgb_irgb();
    }

    fn update_mac123_overflow_flags(&mut self, mac1: i64, mac2: i64, mac3: i64) {
        let flags = &[
            (
                Flag::MAC1_RES_LARGER_THAN_43_BITS_NEG,
                Flag::MAC1_RES_LARGER_THAN_43_BITS_POS,
            ),
            (
                Flag::MAC2_RES_LARGER_THAN_43_BITS_NEG,
                Flag::MAC2_RES_LARGER_THAN_43_BITS_POS,
            ),
            (
                Flag::MAC3_RES_LARGER_THAN_43_BITS_NEG,
                Flag::MAC3_RES_LARGER_THAN_43_BITS_POS,
            ),
        ];

        let mut mac = [mac1, mac2, mac3];

        for (value, (neg_flag, pos_flag)) in mac.iter_mut().zip(flags) {
            if *value < -(1 << 43) {
                self.flag.insert(*neg_flag);
            } else if *value > (1 << 43) - 1 {
                self.flag.insert(*pos_flag);
            }
        }
    }

    fn mac123_sign_extend(&mut self, mac1: i64, mac2: i64, mac3: i64) -> (i64, i64, i64) {
        self.update_mac123_overflow_flags(mac1, mac2, mac3);

        #[inline(always)]
        fn sign_extend(value: i64) -> i64 {
            (((value as u64) << (64 - 43 - 1)) as i64) >> (64 - 43 - 1)
        }

        (sign_extend(mac1), sign_extend(mac2), sign_extend(mac3))
    }

    // check overflow for mac123, shifts the values with sf flag, and return the
    // shifted value but not truncated (i64 value) to be used by later calucations
    // if needed
    fn set_mac123(&mut self, mac1: i64, mac2: i64, mac3: i64, sf: bool) -> (i64, i64, i64) {
        let sf = sf as u32;

        self.update_mac123_overflow_flags(mac1, mac2, mac3);

        let mac1 = mac1.wrapping_shr(sf * 12);
        let mac2 = mac2.wrapping_shr(sf * 12);
        let mac3 = mac3.wrapping_shr(sf * 12);

        self.mac[1] = mac1 as i32;
        self.mac[2] = mac2 as i32;
        self.mac[3] = mac3 as i32;

        (mac1, mac2, mac3)
    }

    fn rgb_mul_ir(&mut self) -> (i64, i64, i64) {
        // [MAC1,MAC2,MAC3] = [R*IR1,G*IR2,B*IR3] SHL 4
        let (r, g, b) = get_rgb(self.rgbc);
        let mac1 = (self.ir[1] as i64 * r) << 4;
        let mac2 = (self.ir[2] as i64 * g) << 4;
        let mac3 = (self.ir[3] as i64 * b) << 4;
        self.mac123_sign_extend(mac1, mac2, mac3)
    }
}

impl Gte {
    fn mvmva(
        &mut self,
        tx: &[i32; 3],
        mx: &[[i16; 3]; 3],
        vx: &[i16; 3],
        sf: bool,
        lm: bool,
    ) -> (i64, i64, i64) {
        // MAC1 = (Tx1*1000h + Mx11*Vx1 + Mx12*Vx2 + Mx13*Vx3) SAR (sf*12)
        // MAC2 = (Tx2*1000h + Mx21*Vx1 + Mx22*Vx2 + Mx23*Vx3) SAR (sf*12)
        // MAC3 = (Tx3*1000h + Mx31*Vx1 + Mx32*Vx2 + Mx33*Vx3) SAR (sf*12)

        let mac1 = (tx[0] as i64).wrapping_shl(12);
        let mac2 = (tx[1] as i64).wrapping_shl(12);
        let mac3 = (tx[2] as i64).wrapping_shl(12);
        let (mac1, mac2, mac3) = self.mac123_sign_extend(mac1, mac2, mac3);

        let mac1 = mac1 + mx[0][0] as i64 * vx[0] as i64;
        let mac2 = mac2 + mx[1][0] as i64 * vx[0] as i64;
        let mac3 = mac3 + mx[2][0] as i64 * vx[0] as i64;
        let (mac1, mac2, mac3) = self.mac123_sign_extend(mac1, mac2, mac3);

        let mac1 = mac1 + mx[0][1] as i64 * vx[1] as i64;
        let mac2 = mac2 + mx[1][1] as i64 * vx[1] as i64;
        let mac3 = mac3 + mx[2][1] as i64 * vx[1] as i64;
        let (mac1, mac2, mac3) = self.mac123_sign_extend(mac1, mac2, mac3);

        let mac1 = mac1 + mx[0][2] as i64 * vx[2] as i64;
        let mac2 = mac2 + mx[1][2] as i64 * vx[2] as i64;
        let mac3 = mac3 + mx[2][2] as i64 * vx[2] as i64;
        let (mac1, mac2, mac3) = self.mac123_sign_extend(mac1, mac2, mac3);

        let (mac1, mac2, mac3) = self.set_mac123(mac1, mac2, mac3, sf);

        self.copy_mac_ir_saturate(lm);

        (mac1, mac2, mac3)
    }

    #[allow(clippy::many_single_char_names)]
    fn rtp_unr_division(&mut self) -> i64 {
        let h = self.projection_plain_distance;
        let sz3 = self.sz[3];

        #[rustfmt::skip]
        const UNR_TABLE: &[u32] = &[
            0xFF, 0xFD, 0xFB, 0xF9, 0xF7, 0xF5, 0xF3, 0xF1, 0xEF, 0xEE, 0xEC, 0xEA, 0xE8, 0xE6, 0xE4, 0xE3, // \
            0xE1, 0xDF, 0xDD, 0xDC, 0xDA, 0xD8, 0xD6, 0xD5, 0xD3, 0xD1, 0xD0, 0xCE, 0xCD, 0xCB, 0xC9, 0xC8, //  0x00..0x3F
            0xC6, 0xC5, 0xC3, 0xC1, 0xC0, 0xBE, 0xBD, 0xBB, 0xBA, 0xB8, 0xB7, 0xB5, 0xB4, 0xB2, 0xB1, 0xB0, //
            0xAE, 0xAD, 0xAB, 0xAA, 0xA9, 0xA7, 0xA6, 0xA4, 0xA3, 0xA2, 0xA0, 0x9F, 0x9E, 0x9C, 0x9B, 0x9A, // /
            0x99, 0x97, 0x96, 0x95, 0x94, 0x92, 0x91, 0x90, 0x8F, 0x8D, 0x8C, 0x8B, 0x8A, 0x89, 0x87, 0x86, // \
            0x85, 0x84, 0x83, 0x82, 0x81, 0x7F, 0x7E, 0x7D, 0x7C, 0x7B, 0x7A, 0x79, 0x78, 0x77, 0x75, 0x74, //  0x40..0x7F
            0x73, 0x72, 0x71, 0x70, 0x6F, 0x6E, 0x6D, 0x6C, 0x6B, 0x6A, 0x69, 0x68, 0x67, 0x66, 0x65, 0x64, //
            0x63, 0x62, 0x61, 0x60, 0x5F, 0x5E, 0x5D, 0x5D, 0x5C, 0x5B, 0x5A, 0x59, 0x58, 0x57, 0x56, 0x55, // /
            0x54, 0x53, 0x53, 0x52, 0x51, 0x50, 0x4F, 0x4E, 0x4D, 0x4D, 0x4C, 0x4B, 0x4A, 0x49, 0x48, 0x48, // \
            0x47, 0x46, 0x45, 0x44, 0x43, 0x43, 0x42, 0x41, 0x40, 0x3F, 0x3F, 0x3E, 0x3D, 0x3C, 0x3C, 0x3B, //  0x80..0xBF
            0x3A, 0x39, 0x39, 0x38, 0x37, 0x36, 0x36, 0x35, 0x34, 0x33, 0x33, 0x32, 0x31, 0x31, 0x30, 0x2F, //
            0x2E, 0x2E, 0x2D, 0x2C, 0x2C, 0x2B, 0x2A, 0x2A, 0x29, 0x28, 0x28, 0x27, 0x26, 0x26, 0x25, 0x24, // /
            0x24, 0x23, 0x22, 0x22, 0x21, 0x20, 0x20, 0x1F, 0x1E, 0x1E, 0x1D, 0x1D, 0x1C, 0x1B, 0x1B, 0x1A, // \
            0x19, 0x19, 0x18, 0x18, 0x17, 0x16, 0x16, 0x15, 0x15, 0x14, 0x14, 0x13, 0x12, 0x12, 0x11, 0x11, //  0xC0..0xFF
            0x10, 0x0F, 0x0F, 0x0E, 0x0E, 0x0D, 0x0D, 0x0C, 0x0C, 0x0B, 0x0A, 0x0A, 0x09, 0x09, 0x08, 0x08, //
            0x07, 0x07, 0x06, 0x06, 0x05, 0x05, 0x04, 0x04, 0x03, 0x03, 0x02, 0x02, 0x01, 0x01, 0x00, 0x00, // /
            0x00, //<-- one extra table entry (for "(d-7F0xC0)/0x80"=10x00)   // -10x00
        ];

        if (h as u32) < (sz3 as u32) * 2 {
            let z = sz3.leading_zeros();
            let n = (h as u32) << z;
            let d = sz3.wrapping_shl(z) as u32;
            let u = UNR_TABLE[((d - 0x7FC0) >> 7) as usize] + 0x101;
            let d = (0x2000080 - (d * u)) >> 8;
            let d = (0x0000080 + (d * u)) >> 8;

            let n = ((n as u64 * d as u64 + 0x8000) >> 16).min(0x1FFFF);

            n as i64
        } else {
            self.flag.insert(Flag::DIVIDE_OVERFLOW);
            0x1FFFF
        }
    }

    fn push_color_fifo_from_mac123(&mut self, mac1: i64, mac2: i64, mac3: i64, sf: bool, lm: bool) {
        // [MAC1,MAC2,MAC3] = [MAC1,MAC2,MAC3] SAR (sf*12)
        // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]

        let code = ((self.rgbc >> 24) & 0xFF) as u8;

        // [MAC1,MAC2,MAC3] = [MAC1,MAC2,MAC3] SAR (sf*12)
        self.set_mac123(mac1, mac2, mac3, sf);

        // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]
        self.push_color_fifo(self.mac[1] >> 4, self.mac[2] >> 4, self.mac[3] >> 4, code);
        self.copy_mac_ir_saturate(lm);
    }

    /// A method that handles the end part of commands such as:
    /// Dcpl, Dpcs, Dpct, Intpl, Ncd*, Cdp
    fn color_interpolation(&mut self, mac1: i64, mac2: i64, mac3: i64, sf: bool, lm: bool) {
        // [MAC1,MAC2,MAC3] = [X, X, X]     ; input depending on the command
        // [MAC1,MAC2,MAC3] = MAC+(FC-MAC)*IR0
        // [MAC1,MAC2,MAC3] = [MAC1,MAC2,MAC3] SAR (sf*12)
        // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]

        // [MAC1,MAC2,MAC3] = MAC+(FC-MAC)*IR0
        //
        // (FC-MAC)
        // [IR1,IR2,IR3] = (([RFC,GFC,BFC] SHL 12) - [MAC1,MAC2,MAC3]) SAR (sf*12)
        let tmp_mac1 = (self.far_color[0] as i64).wrapping_shl(12) - mac1;
        let tmp_mac2 = (self.far_color[1] as i64).wrapping_shl(12) - mac2;
        let tmp_mac3 = (self.far_color[2] as i64).wrapping_shl(12) - mac3;

        self.set_mac123(tmp_mac1, tmp_mac2, tmp_mac3, sf);

        self.copy_mac_ir_saturate(false);

        // *IR0+MAC
        // [MAC1,MAC2,MAC3] = (([IR1,IR2,IR3] * IR0) + [MAC1,MAC2,MAC3])
        let mac1 = (self.ir[1] as i64 * self.ir[0] as i64) + mac1;
        let mac2 = (self.ir[2] as i64 * self.ir[0] as i64) + mac2;
        let mac3 = (self.ir[3] as i64 * self.ir[0] as i64) + mac3;

        // [MAC1,MAC2,MAC3] = [MAC1,MAC2,MAC3] SAR (sf*12)
        // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]
        self.push_color_fifo_from_mac123(mac1, mac2, mac3, sf, lm);
    }

    fn rtps(&mut self, v_index: usize, sf: bool, lm: bool, triple: bool, last: bool) {
        // IR1 = MAC1 = (TRX*1000h + RT11*VX0 + RT12*VY0 + RT13*VZ0) SAR (sf*12)
        // IR2 = MAC2 = (TRY*1000h + RT21*VX0 + RT22*VY0 + RT23*VZ0) SAR (sf*12)
        // IR3 = MAC3 = (TRZ*1000h + RT31*VX0 + RT32*VY0 + RT33*VZ0) SAR (sf*12)
        // SZ3 = MAC3 SAR ((1-sf)*12)
        // MAC0=(((H*20000h/SZ3)+1)/2)*IR1+OFX, SX2=MAC0/10000h
        // MAC0=(((H*20000h/SZ3)+1)/2)*IR2+OFY, SY2=MAC0/10000h
        // MAC0=(((H*20000h/SZ3)+1)/2)*DQA+DQB, IR0=MAC0/1000h

        let tx = self.translation_vector;
        let mx = self.rotation_matrix;
        let vx = self.vectors[v_index];

        // IR1 = MAC1 = (TRX*1000h + RT11*VX0 + RT12*VY0 + RT13*VZ0) SAR (sf*12)
        // IR2 = MAC2 = (TRY*1000h + RT21*VX0 + RT22*VY0 + RT23*VZ0) SAR (sf*12)
        // IR3 = MAC3 = (TRZ*1000h + RT31*VX0 + RT32*VY0 + RT33*VZ0) SAR (sf*12)
        let (_mac1, _mac2, mac3) = self.mvmva(&tx, &mx, &vx, sf, lm);

        // When using RTPS command with sf=0, then the IR3 saturation
        // flag (FLAG.22) gets set only if "MAC3 SAR 12" exceeds -8000h..+7FFFh
        if !sf && !triple {
            self.flag
                .remove(Flag::IR3_SATURATED_TO_P0000_P7FFF_OR_N8000_P7FFF);

            let shifted_mac3 = self.mac[3].wrapping_shr(12);

            if !(-0x8000..=0x7FFF).contains(&shifted_mac3) {
                self.flag
                    .insert(Flag::IR3_SATURATED_TO_P0000_P7FFF_OR_N8000_P7FFF);
            }
        }

        // SZ3 = MAC3 SAR ((1-sf)*12)
        let sz = self.saturate_put_flag(
            mac3.wrapping_shr((1 - sf as u32) * 12) as i32,
            0,
            0xFFFF,
            Flag::SZ3_OR_OTZ_SATURATED_TO_0000_FFFF,
        ) as u16;
        self.push_sz_fifo(sz);

        // this value is used 3 times, so we cache it here
        let n = self.rtp_unr_division();

        // MAC0=(((H*20000h/SZ3)+1)/2)*IR1+OFX, SX2=MAC0/10000h
        let mac0 = n * self.ir[1] as i64 + self.screen_offset[0] as i64;
        self.set_mac0(mac0);

        let sx = self.saturate_put_flag(
            (mac0 >> 16) as i32,
            -0x400,
            0x3FF,
            Flag::SX2_SATURATED_TO_N0400_P03FF,
        ) as i16;

        // MAC0=(((H*20000h/SZ3)+1)/2)*IR2+OFY, SY2=MAC0/10000h
        let mac0 = n * self.ir[2] as i64 + self.screen_offset[1] as i64;
        self.set_mac0(mac0);

        let sy = self.saturate_put_flag(
            (mac0 >> 16) as i32,
            -0x400,
            0x3FF,
            Flag::SY2_SATURATED_TO_N0400_P03FF,
        ) as i16;
        self.push_sxy_fifo(sx, sy);

        if last {
            // MAC0=(((H*20000h/SZ3)+1)/2)*DQA+DQB, IR0=MAC0/1000h
            let mac0 = n * self.dqa as i64 + self.dqb as i64;
            self.set_mac0(mac0);

            self.set_ir0((mac0 >> 12) as i32);
        }
    }

    fn nc_common_start(&mut self, v_index: usize, sf: bool, lm: bool) {
        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (LLM*Vx) SAR (sf*12)
        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (BK*1000h + LCM*IR) SAR (sf*12)

        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (LLM*V0) SAR (sf*12)
        let vx = self.vectors[v_index];
        let mx = self.light_source_matrix;
        let tx = [0; 3];
        self.mvmva(&tx, &mx, &vx, sf, lm);

        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (BK*1000h + LCM*IR) SAR (sf*12)
        let vx = [self.ir[1], self.ir[2], self.ir[3]];
        let mx = self.light_color_matrix;
        let tx = self.background_color;
        self.mvmva(&tx, &mx, &vx, sf, lm);
    }

    fn ncds_nccs_common(&mut self, v_index: usize, sf: bool, lm: bool) -> (i64, i64, i64) {
        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (LLM*Vx) SAR (sf*12)
        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (BK*1000h + LCM*IR) SAR (sf*12)
        // [MAC1,MAC2,MAC3] = [R*IR1,G*IR2,B*IR3] SHL 4

        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (LLM*Vx) SAR (sf*12)
        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (BK*1000h + LCM*IR) SAR (sf*12)
        self.nc_common_start(v_index, sf, lm);

        // [MAC1,MAC2,MAC3] = [R*IR1,G*IR2,B*IR3] SHL 4
        self.rgb_mul_ir()
    }

    fn ncds(&mut self, v_index: usize, sf: bool, lm: bool) {
        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (LLM*Vx) SAR (sf*12)
        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (BK*1000h + LCM*IR) SAR (sf*12)
        // [MAC1,MAC2,MAC3] = [R*IR1,G*IR2,B*IR3] SHL 4
        // [MAC1,MAC2,MAC3] = MAC+(FC-MAC)*IR0
        // [MAC1,MAC2,MAC3] = [MAC1,MAC2,MAC3] SAR (sf*12)
        // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]

        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (LLM*Vx) SAR (sf*12)
        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (BK*1000h + LCM*IR) SAR (sf*12)
        // [MAC1,MAC2,MAC3] = [R*IR1,G*IR2,B*IR3] SHL 4
        let (mac1, mac2, mac3) = self.ncds_nccs_common(v_index, sf, lm);

        // [MAC1,MAC2,MAC3] = MAC+(FC-MAC)*IR0
        // [MAC1,MAC2,MAC3] = [MAC1,MAC2,MAC3] SAR (sf*12)
        // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]
        self.color_interpolation(mac1, mac2, mac3, sf, lm);
    }

    fn nccs(&mut self, v_index: usize, sf: bool, lm: bool) {
        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (LLM*Vx) SAR (sf*12)
        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (BK*1000h + LCM*IR) SAR (sf*12)
        // [MAC1,MAC2,MAC3] = [R*IR1,G*IR2,B*IR3] SHL 4
        // [MAC1,MAC2,MAC3] = [MAC1,MAC2,MAC3] SAR (sf*12)
        // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]

        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (LLM*Vx) SAR (sf*12)
        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (BK*1000h + LCM*IR) SAR (sf*12)
        // [MAC1,MAC2,MAC3] = [R*IR1,G*IR2,B*IR3] SHL 4
        let (mac1, mac2, mac3) = self.ncds_nccs_common(v_index, sf, lm);

        // [MAC1,MAC2,MAC3] = [MAC1,MAC2,MAC3] SAR (sf*12)
        // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]
        self.push_color_fifo_from_mac123(mac1, mac2, mac3, sf, lm);
    }

    fn ncs(&mut self, v_index: usize, sf: bool, lm: bool) {
        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (LLM*V0) SAR (sf*12)
        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (BK*1000h + LCM*IR) SAR (sf*12)
        // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]

        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (LLM*Vx) SAR (sf*12)
        // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (BK*1000h + LCM*IR) SAR (sf*12)
        self.nc_common_start(v_index, sf, lm);

        // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]
        let code = ((self.rgbc >> 24) & 0xFF) as u8;
        self.push_color_fifo(self.mac[1] >> 4, self.mac[2] >> 4, self.mac[3] >> 4, code);
    }

    fn dpcs(&mut self, sf: bool, lm: bool, rgb: u32) {
        let (r, g, b) = get_rgb(rgb);

        // [MAC1,MAC2,MAC3] = [R,G,B] SHL 16
        let mac1 = r << 16;
        let mac2 = g << 16;
        let mac3 = b << 16;
        let (mac1, mac2, mac3) = self.mac123_sign_extend(mac1, mac2, mac3);

        // [MAC1,MAC2,MAC3] = MAC+(FC-MAC)*IR0
        // [MAC1,MAC2,MAC3] = [MAC1,MAC2,MAC3] SAR (sf*12)
        // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]
        self.color_interpolation(mac1, mac2, mac3, sf, lm);
    }

    fn gpf(&mut self, mac1: i64, mac2: i64, mac3: i64, sf: bool, lm: bool) {
        // [MAC1,MAC2,MAC3] = (([IR1,IR2,IR3] * IR0) + [MAC1,MAC2,MAC3])
        let mac1 = (self.ir[1] as i64 * self.ir[0] as i64) + mac1;
        let mac2 = (self.ir[2] as i64 * self.ir[0] as i64) + mac2;
        let mac3 = (self.ir[3] as i64 * self.ir[0] as i64) + mac3;
        let (mac1, mac2, mac3) = self.mac123_sign_extend(mac1, mac2, mac3);

        // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]
        self.push_color_fifo_from_mac123(mac1, mac2, mac3, sf, lm);
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
            13..=15 => self.background_color[num as usize - 13] as u32,
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
            21..=23 => self.far_color[num as usize - 21] as u32,
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
            13..=15 => self.background_color[num as usize - 13] = data as i32,
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
            21..=23 => self.far_color[num as usize - 21] = data as i32,
            24 => self.screen_offset[0] = data as i32,
            25 => self.screen_offset[1] = data as i32,
            26 => self.projection_plain_distance = data as u16,
            27 => self.dqa = (data & 0xFFFF) as i16,
            28 => self.dqb = data as i32,
            29 => self.zsf3 = data as i16,
            30 => self.zsf4 = data as i16,
            31 => self.flag = Flag::from_bits_retain(data),
            _ => unreachable!(),
        }
    }

    pub fn execute_command(&mut self, cmd_word: u32) {
        // clear before start of command
        self.flag = Flag::empty();

        let cmd = GteCommand::from_u32(cmd_word);

        log::info!("cop2 executing command {:?}", cmd);

        match cmd.opcode {
            GteCommandOpcode::Na => {
                println!("WARN: GTE: unknown command command, cmd: {:08X}", cmd_word);
            }
            GteCommandOpcode::Rtps => {
                self.rtps(0, cmd.sf, cmd.lm, false, true);
            }
            GteCommandOpcode::Rtpt => {
                self.rtps(0, cmd.sf, cmd.lm, true, false);
                self.rtps(1, cmd.sf, cmd.lm, true, false);
                self.rtps(2, cmd.sf, cmd.lm, true, true);
            }
            GteCommandOpcode::Mvmva => {
                // MAC1 = (Tx1*1000h + Mx11*Vx1 + Mx12*Vx2 + Mx13*Vx3) SAR (sf*12)
                // MAC2 = (Tx2*1000h + Mx21*Vx1 + Mx22*Vx2 + Mx23*Vx3) SAR (sf*12)
                // MAC3 = (Tx3*1000h + Mx31*Vx1 + Mx32*Vx2 + Mx33*Vx3) SAR (sf*12)
                // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]

                let mx = match cmd.mx {
                    0 => self.rotation_matrix,
                    1 => self.light_source_matrix,
                    2 => self.light_color_matrix,
                    3 => {
                        let r = (self.rgbc & 0xFF) as i16;
                        [
                            [-(r << 4), r << 4, self.ir[0]],
                            [self.rotation_matrix[0][2]; 3],
                            [self.rotation_matrix[1][1]; 3],
                        ]
                    }
                    _ => unreachable!(),
                };

                let mut tx = match cmd.tx {
                    0 => self.translation_vector,
                    1 => self.background_color,
                    2 => self.far_color,
                    3 => [0; 3], // none
                    _ => unreachable!(),
                };

                let mut vx = [0; 3];
                match cmd.vx {
                    0..=2 => vx = self.vectors[cmd.vx as usize],
                    3 => vx.copy_from_slice(&self.ir[1..]),
                    _ => unreachable!(),
                }

                if cmd.tx == 2 {
                    // If far_color is selected, then a bug occured where we check
                    // for the flag with the calculation MAC1=(Tx1*1000h + Mx11*Vx1)
                    // but the values are not returned, so we use the calculation only
                    // to set the flags, then perform it again with another modification
                    // to get the returned result
                    self.mvmva(&tx, &mx, &[vx[0], 0, 0], cmd.sf, cmd.lm);

                    // the return values are reduced to:
                    // MAC1=(Mx12*Vx2 + Mx13*Vx3) SAR (sf*12), and similar for
                    // MAC2 and MAC3.
                    //
                    // We can achieve that by zeroing out the tx vector and vx1
                    // from the vx vector
                    tx = [0; 3];
                    vx[0] = 0;
                }
                // here, if `cmd.tx != 2`, mvmva will run only once,
                // but if `cmd.tx == 2` it will run the previous if statement
                // along with this mvmva
                self.mvmva(&tx, &mx, &vx, cmd.sf, cmd.lm);
            }
            GteCommandOpcode::Dcpl => {
                // [MAC1,MAC2,MAC3] = [R*IR1,G*IR2,B*IR3] SHL 4
                let (mac1, mac2, mac3) = self.rgb_mul_ir();

                // [MAC1,MAC2,MAC3] = MAC+(FC-MAC)*IR0
                // [MAC1,MAC2,MAC3] = [MAC1,MAC2,MAC3] SAR (sf*12)
                // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]
                self.color_interpolation(mac1, mac2, mac3, cmd.sf, cmd.lm);
            }
            GteCommandOpcode::Dpcs => {
                self.dpcs(cmd.sf, cmd.lm, self.rgbc);
            }
            GteCommandOpcode::Dpct => {
                self.dpcs(cmd.sf, cmd.lm, self.rgb[0]);
                self.dpcs(cmd.sf, cmd.lm, self.rgb[0]);
                self.dpcs(cmd.sf, cmd.lm, self.rgb[0]);
            }
            GteCommandOpcode::Intpl => {
                // [MAC1,MAC2,MAC3] = [IR1,IR2,IR3] SHL 12
                let mac1 = (self.ir[1] as i64).wrapping_shl(12);
                let mac2 = (self.ir[2] as i64).wrapping_shl(12);
                let mac3 = (self.ir[3] as i64).wrapping_shl(12);
                let (mac1, mac2, mac3) = self.mac123_sign_extend(mac1, mac2, mac3);

                // [MAC1,MAC2,MAC3] = MAC+(FC-MAC)*IR0
                // [MAC1,MAC2,MAC3] = [MAC1,MAC2,MAC3] SAR (sf*12)
                // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]
                self.color_interpolation(mac1, mac2, mac3, cmd.sf, cmd.lm);
            }
            GteCommandOpcode::Sqr => {
                // [MAC1,MAC2,MAC3] = [IR1*IR1,IR2*IR2,IR3*IR3] SHR (sf*12)
                // [IR1,IR2,IR3]    = [MAC1,MAC2,MAC3]    ;IR1,IR2,IR3 saturated to max 7FFFh

                let mac1 = self.ir[1] as i64 * self.ir[1] as i64;
                let mac2 = self.ir[2] as i64 * self.ir[2] as i64;
                let mac3 = self.ir[3] as i64 * self.ir[3] as i64;
                self.set_mac123(mac1, mac2, mac3, cmd.sf);
                self.copy_mac_ir_saturate(cmd.lm);
            }
            GteCommandOpcode::Ncs => {
                self.ncs(0, cmd.sf, cmd.lm);
            }
            GteCommandOpcode::Nct => {
                self.ncs(0, cmd.sf, cmd.lm);
                self.ncs(1, cmd.sf, cmd.lm);
                self.ncs(2, cmd.sf, cmd.lm);
            }
            GteCommandOpcode::Ncds => {
                self.ncds(0, cmd.sf, cmd.lm);
            }
            GteCommandOpcode::Ncdt => {
                self.ncds(0, cmd.sf, cmd.lm);
                self.ncds(1, cmd.sf, cmd.lm);
                self.ncds(2, cmd.sf, cmd.lm);
            }
            GteCommandOpcode::Nccs => {
                self.nccs(0, cmd.sf, cmd.lm);
            }
            GteCommandOpcode::Ncct => {
                self.nccs(0, cmd.sf, cmd.lm);
                self.nccs(1, cmd.sf, cmd.lm);
                self.nccs(2, cmd.sf, cmd.lm);
            }
            GteCommandOpcode::Cdp => {
                // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (BK*1000h + LCM*IR) SAR (sf*12)
                // [MAC1,MAC2,MAC3] = [R*IR1,G*IR2,B*IR3] SHL 4
                // [MAC1,MAC2,MAC3] = MAC+(FC-MAC)*IR0
                // [MAC1,MAC2,MAC3] = [MAC1,MAC2,MAC3] SAR (sf*12)
                // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]

                // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (BK*1000h + LCM*IR) SAR (sf*12)
                let vx = [self.ir[1], self.ir[2], self.ir[3]];
                let mx = self.light_color_matrix;
                let tx = self.background_color;
                self.mvmva(&tx, &mx, &vx, cmd.sf, cmd.lm);

                // [MAC1,MAC2,MAC3] = [R*IR1,G*IR2,B*IR3] SHL 4
                let (mac1, mac2, mac3) = self.rgb_mul_ir();

                // [MAC1,MAC2,MAC3] = MAC+(FC-MAC)*IR0
                // [MAC1,MAC2,MAC3] = [MAC1,MAC2,MAC3] SAR (sf*12)
                // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]
                self.color_interpolation(mac1, mac2, mac3, cmd.sf, cmd.lm);
            }
            GteCommandOpcode::Cc => {
                // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (BK*1000h + LCM*IR) SAR (sf*12)
                // [MAC1,MAC2,MAC3] = [R*IR1,G*IR2,B*IR3] SHL 4
                // [MAC1,MAC2,MAC3] = [MAC1,MAC2,MAC3] SAR (sf*12)
                // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]

                // [IR1,IR2,IR3] = [MAC1,MAC2,MAC3] = (BK*1000h + LCM*IR) SAR (sf*12)
                let vx = [self.ir[1], self.ir[2], self.ir[3]];
                let mx = self.light_color_matrix;
                let tx = self.background_color;
                self.mvmva(&tx, &mx, &vx, cmd.sf, cmd.lm);

                // [MAC1,MAC2,MAC3] = [R*IR1,G*IR2,B*IR3] SHL 4
                let (mac1, mac2, mac3) = self.rgb_mul_ir();

                // [MAC1,MAC2,MAC3] = [MAC1,MAC2,MAC3] SAR (sf*12)
                // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]
                self.push_color_fifo_from_mac123(mac1, mac2, mac3, cmd.sf, cmd.lm);
            }
            GteCommandOpcode::Nclip => {
                // MAC0 = SX0*SY1 + SX1*SY2 + SX2*SY0 - SX0*SY2 - SX1*SY0 - SX2*SY1
                let mac0 = self.sxy[0].0 as i64 * self.sxy[1].1 as i64
                    + self.sxy[1].0 as i64 * self.sxy[2].1 as i64
                    + self.sxy[2].0 as i64 * self.sxy[0].1 as i64
                    - self.sxy[0].0 as i64 * self.sxy[2].1 as i64
                    - self.sxy[1].0 as i64 * self.sxy[0].1 as i64
                    - self.sxy[2].0 as i64 * self.sxy[1].1 as i64;
                self.set_mac0(mac0);
            }
            GteCommandOpcode::Avsz3 => {
                // MAC0 =  ZSF3*(SZ1+SZ2+SZ3)
                // OTZ  =  MAC0/1000h               ;(saturated to 0..FFFFh)

                let mac0 =
                    self.zsf3 as i64 * (self.sz[1] as i64 + self.sz[2] as i64 + self.sz[3] as i64);
                self.set_mac0(mac0);

                self.otz = self.saturate_put_flag(
                    mac0 >> 12,
                    0,
                    0xFFFF,
                    Flag::SZ3_OR_OTZ_SATURATED_TO_0000_FFFF,
                ) as u16;
            }
            GteCommandOpcode::Avsz4 => {
                // MAC0 =  ZSF4*(SZ0+SZ1+SZ2+SZ3)
                // OTZ  =  MAC0/1000h               ; (saturated to 0..FFFFh)

                let mac0 = self.zsf4 as i64
                    * (self.sz[0] as i64
                        + self.sz[1] as i64
                        + self.sz[2] as i64
                        + self.sz[3] as i64);
                self.set_mac0(mac0);

                self.otz = self.saturate_put_flag(
                    mac0 >> 12,
                    0,
                    0xFFFF,
                    Flag::SZ3_OR_OTZ_SATURATED_TO_0000_FFFF,
                ) as u16;
            }
            GteCommandOpcode::Op => {
                // [MAC1,MAC2,MAC3] = [IR3*D2-IR2*D3, IR1*D3-IR3*D1, IR2*D1-IR1*D2] SAR (sf*12)
                // [IR1,IR2,IR3]    = [MAC1,MAC2,MAC3]

                // [MAC1,MAC2,MAC3] = [IR3*D2-IR2*D3, IR1*D3-IR3*D1, IR2*D1-IR1*D2] SAR (sf*12)
                let d = [
                    self.rotation_matrix[0][0],
                    self.rotation_matrix[1][1],
                    self.rotation_matrix[2][2],
                ];
                let mac1 = self.ir[3] as i64 * d[1] as i64;
                let mac2 = self.ir[1] as i64 * d[2] as i64;
                let mac3 = self.ir[2] as i64 * d[0] as i64;
                let (mac1, mac2, mac3) = self.mac123_sign_extend(mac1, mac2, mac3);
                let mac1 = mac1 - self.ir[2] as i64 * d[2] as i64;
                let mac2 = mac2 - self.ir[3] as i64 * d[0] as i64;
                let mac3 = mac3 - self.ir[1] as i64 * d[1] as i64;

                self.set_mac123(mac1, mac2, mac3, cmd.sf);

                // [IR1,IR2,IR3]    = [MAC1,MAC2,MAC3]
                self.copy_mac_ir_saturate(cmd.lm);
            }
            GteCommandOpcode::Gpf => {
                // [MAC1,MAC2,MAC3] = [0,0,0]
                // [MAC1,MAC2,MAC3] = (([IR1,IR2,IR3] * IR0) + [MAC1,MAC2,MAC3]) SAR (sf*12)
                // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]

                self.gpf(0, 0, 0, cmd.sf, cmd.lm);
            }
            GteCommandOpcode::Gpl => {
                // [MAC1,MAC2,MAC3] = [MAC1,MAC2,MAC3] SHL (sf*12)
                // [MAC1,MAC2,MAC3] = (([IR1,IR2,IR3] * IR0) + [MAC1,MAC2,MAC3]) SAR (sf*12)
                // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]

                let sf = cmd.sf as u32;

                let mac1 = (self.mac[1] as i64) << (sf * 12);
                let mac2 = (self.mac[2] as i64) << (sf * 12);
                let mac3 = (self.mac[3] as i64) << (sf * 12);
                let (mac1, mac2, mac3) = self.mac123_sign_extend(mac1, mac2, mac3);

                // [MAC1,MAC2,MAC3] = (([IR1,IR2,IR3] * IR0) + [MAC1,MAC2,MAC3]) SAR (sf*12)
                // Color FIFO = [MAC1/16,MAC2/16,MAC3/16,CODE], [IR1,IR2,IR3] = [MAC1,MAC2,MAC3]
                self.gpf(mac1, mac2, mac3, cmd.sf, cmd.lm);
            }
        }
    }
}
