use std::collections::VecDeque;

use crate::memory::BusLine;
use bitflags::bitflags;
use byteorder::{ByteOrder, LittleEndian};

// reversed ZIGZAG order
const ZAGZIG: [usize; 64] = [
    63, 62, 58, 57, 49, 48, 36, 35, 61, 59, 56, 50, 47, 37, 34, 21, 60, 55, 51, 46, 38, 33, 22, 20,
    54, 52, 45, 39, 32, 23, 19, 10, 53, 44, 40, 31, 24, 18, 11, 9, 43, 41, 30, 25, 17, 12, 8, 3,
    42, 29, 26, 16, 13, 7, 4, 2, 28, 27, 15, 14, 6, 5, 1, 0,
];

const fn extend_sign<const N: usize>(x: u16) -> i32 {
    let mask: u32 = (1 << N) - 1;
    let x = x as u32 & mask;
    let sign_extend = (0xFFFFFFFFu32 - mask) * ((x >> (N - 1)) & 1);

    (x | sign_extend) as i32
}

bitflags! {
    #[derive(Default)]
    pub struct MdecStatus: u32 {
        const DATA_OUT_FIFO_EMPTY     = 0b1000_0000_0000_0000_0000_0000_0000_0000;
        const DATA_IN_FIFO_FULL       = 0b0100_0000_0000_0000_0000_0000_0000_0000;
        const COMMAND_BUSY            = 0b0010_0000_0000_0000_0000_0000_0000_0000;
        const DATA_IN_REQUEST         = 0b0001_0000_0000_0000_0000_0000_0000_0000;
        const DATA_OUT_REQUEST        = 0b0000_1000_0000_0000_0000_0000_0000_0000;
        const DATA_OUTPUT_DEPTH       = 0b0000_0110_0000_0000_0000_0000_0000_0000;
        const DATA_SIGNED             = 0b0000_0001_0000_0000_0000_0000_0000_0000;
        const DATA_OUTPUT_BIT15_SET   = 0b0000_0000_1000_0000_0000_0000_0000_0000;
        const CURRENT_BLOCK           = 0b0000_0000_0000_0111_0000_0000_0000_0000;
        //const UNUSED_BITS           = 0b0000_0000_0111_1000_0000_0000_0000_0000;
        //const PARAMS_REMAINING      = 0b0000_0000_0000_0000_1111_1111_1111_1111;
    }
}

impl MdecStatus {
    fn output_depth(&self) -> u8 {
        ((self.bits & Self::DATA_OUTPUT_DEPTH.bits) >> 25) as u8
    }

    fn set_current_block(&mut self, block: u8) {
        self.remove(Self::CURRENT_BLOCK);
        self.bits |= ((block as u32) << 16) & Self::CURRENT_BLOCK.bits;
    }
}

enum MdecCommand {
    DecodeMacroBlock {
        data: Vec<u16>,
    },
    SetQuantTable {
        color_and_luminance: bool,
        data: [u8; 128],
    },
    SetScaleTable {
        data: [u16; 64],
    },
}

pub struct Mdec {
    status: MdecStatus,
    remaining_params: u16,
    current_cmd: Option<MdecCommand>,
    params_ptr: usize,

    data_out_fifo: VecDeque<u32>,

    iq_y: [u8; 64],
    iq_uv: [u8; 64],
    scaletable: [u16; 64],
}

impl Default for Mdec {
    fn default() -> Self {
        Self {
            status: MdecStatus::default(),
            remaining_params: 0,
            current_cmd: None,
            params_ptr: 0,

            data_out_fifo: VecDeque::new(),

            iq_y: [0; 64],
            iq_uv: [0; 64],
            scaletable: [0; 64],
        }
    }
}

impl Mdec {
    fn y_to_mono(&self, src: &[i16; 64]) -> [u32; 64] {
        let mut out = [0; 64];
        for i in 0..64 {
            let mut y = extend_sign::<10>(src[i] as u16);
            y = y.clamp(-128, 127);
            if !self.status.intersects(MdecStatus::DATA_SIGNED) {
                y += 128;
            }
            out[i] = (y & 0xFF) as u32;
        }
        out
    }

    fn yuv_to_rgb(
        &self,
        cr_blk: &[i16; 64],
        cb_blk: &[i16; 64],
        y_blk: &[i16; 64],
        xx: usize,
        yy: usize,
        out: &mut [u32; 256],
    ) {
        for y in 0..8 {
            for x in 0..8 {
                let r = cr_blk[((x + xx) / 2) + (((y + yy) / 2) * 8)];
                let b = cb_blk[((x + xx) / 2) + (((y + yy) / 2) * 8)];
                let g = ((r as f32 * -0.3437) + (b as f32 * -0.3437)) as i16;

                let r = (r as f32 * 1.402) as i16;
                let b = (b as f32 * 1.772) as i16;

                let y_data = y_blk[x + y * 8];

                let mut r = (y_data + r).clamp(-128, 127);
                let mut g = (y_data + g).clamp(-128, 127);
                let mut b = (y_data + b).clamp(-128, 127);

                if !self.status.intersects(MdecStatus::DATA_SIGNED) {
                    r += 128;
                    g += 128;
                    b += 128;
                }

                out[(x + xx) + ((y + yy) * 16)] =
                    (r as u16 as u32) | ((g as u16 as u32) << 8) | ((b as u16 as u32) << 16);
            }
        }
    }

    fn real_idct_core(&self, inp_out: &mut [i16; 64]) {
        let mut tmp = [0; 64];

        // pass 1
        for x in 0..8 {
            for y in 0..8 {
                let mut sum = 0;
                for z in 0..8 {
                    sum += inp_out[x + z * 8] as i64 * (self.scaletable[y + z * 8] as i16) as i64;
                }
                tmp[x + y * 8] = sum;
            }
        }

        // pass 2
        for x in 0..8 {
            for y in 0..8 {
                let mut sum = 0;
                for z in 0..8 {
                    sum += tmp[y * 8 + z] * (self.scaletable[x + z * 8] as i16) as i64;
                }
                let t = extend_sign::<9>(((sum >> 32) + ((sum >> 31) & 1)) as u16);
                let t = t.clamp(-128, 127);
                inp_out[x + y * 8] = t as i16;
            }
        }
    }

    // return the decoded data and the next index to start `src` from next
    fn rl_decode_block(&self, src: &[u16], qt: &[u8; 64]) -> ([i16; 64], usize) {
        let mut out = [0; 64];

        let mut i = 0;

        loop {
            let n = src[i];
            if n != 0xFE00 {
                break;
            }
            i += 1;
        }

        let mut n = src[i];
        i += 1;

        let mut k = 0;
        let q_scale = (n >> 10) & 0x3F;
        let mut val = extend_sign::<10>(n & 0x3FF) * qt[k] as i32;

        loop {
            if q_scale == 0 {
                val = extend_sign::<10>(n & 0x3FF) * 2;
            }
            val = val.clamp(-0x400, 0x3FF);
            let index = if q_scale == 0 { k } else { ZAGZIG[k] };
            out[index] = val as i16;

            n = src[i];
            i += 1;

            k += ((n >> 10) & 0x3F) as usize + 1;
            if k > 63 {
                break;
            }

            val = (extend_sign::<10>(n & 0x3FF) * qt[k] as i32 * q_scale as i32 + 4) / 8;
        }

        self.real_idct_core(&mut out);
        (out, i)
    }

    fn exec_current_cmd(&mut self) {
        match self.current_cmd.take() {
            Some(MdecCommand::DecodeMacroBlock { data }) => match self.status.output_depth() {
                0 | 1 => {
                    let (out, _i) = self.rl_decode_block(&data, &self.iq_y);
                    let out = self.y_to_mono(&out);
                    self.status.set_current_block(4); // y
                    self.push_to_out_fifo(&out);
                }
                2 | 3 => {
                    let mut out = [0u32; 256];

                    let mut offset = 0;

                    let (cr_blk, i) = self.rl_decode_block(&data[offset..], &self.iq_uv);
                    offset += i;
                    let (cb_blk, i) = self.rl_decode_block(&data[offset..], &self.iq_uv);
                    offset += i;

                    let (y1, i) = self.rl_decode_block(&data[offset..], &self.iq_y);
                    offset += i;
                    self.yuv_to_rgb(&cr_blk, &cb_blk, &y1, 0, 0, &mut out);

                    let (y2, i) = self.rl_decode_block(&data[offset..], &self.iq_y);
                    offset += i;
                    self.yuv_to_rgb(&cr_blk, &cb_blk, &y2, 0, 8, &mut out);

                    let (y3, i) = self.rl_decode_block(&data[offset..], &self.iq_y);
                    offset += i;
                    self.yuv_to_rgb(&cr_blk, &cb_blk, &y3, 8, 0, &mut out);

                    let (y4, _i) = self.rl_decode_block(&data[offset..], &self.iq_y);
                    //offset += i;
                    self.yuv_to_rgb(&cr_blk, &cb_blk, &y4, 8, 8, &mut out);

                    self.push_to_out_fifo(&out);
                }
                _ => unreachable!(),
            },
            Some(MdecCommand::SetQuantTable {
                color_and_luminance,
                data,
            }) => {
                self.iq_y.copy_from_slice(&data[0..64]);
                if color_and_luminance {
                    self.iq_uv.copy_from_slice(&data[64..128]);
                }
            }
            Some(MdecCommand::SetScaleTable { data }) => {
                self.scaletable = data;
            }
            None => {}
        }
    }
}

impl Mdec {
    fn push_to_out_fifo(&mut self, data: &[u32]) {
        self.status.remove(MdecStatus::DATA_OUT_FIFO_EMPTY);
        match self.status.output_depth() {
            0 => {
                for d in data[..64].chunks(8) {
                    self.data_out_fifo.push_back(u32::from_le_bytes([
                        (d[0] as u8 >> 4) | (d[1] as u8 & 0xF0),
                        (d[2] as u8 >> 4) | (d[3] as u8 & 0xF0),
                        (d[4] as u8 >> 4) | (d[5] as u8 & 0xF0),
                        (d[6] as u8 >> 4) | (d[7] as u8 & 0xF0),
                    ]));
                }
            }
            1 => {
                for d in data[..64].chunks(4) {
                    self.data_out_fifo.push_back(u32::from_le_bytes([
                        d[0] as u8, d[1] as u8, d[2] as u8, d[3] as u8,
                    ]));
                }
            }
            2 => todo!("depth 2"),
            3 => {
                let bit_15 = self.status.intersects(MdecStatus::DATA_OUTPUT_BIT15_SET) as u16;
                for d in data[..256].chunks(2) {
                    let [r, g, b, _] = d[0].to_le_bytes();
                    let r = r >> 3;
                    let g = g >> 3;
                    let b = b >> 3;
                    let d1 = (r as u16) | ((g as u16) << 5) | ((b as u16) << 10) | (bit_15 << 15);

                    let [r, g, b, _] = d[1].to_le_bytes();
                    let r = r >> 3;
                    let g = g >> 3;
                    let b = b >> 3;
                    let d2 = (r as u16) | ((g as u16) << 5) | ((b as u16) << 10) | (bit_15 << 15);

                    self.data_out_fifo
                        .push_back((d1 as u32) | ((d2 as u32) << 16));
                }
            }
            _ => unreachable!(),
        }
    }

    fn read_response(&mut self) -> u32 {
        if let Some(out) = self.data_out_fifo.pop_front() {
            if self.data_out_fifo.is_empty() {
                self.status.insert(MdecStatus::DATA_OUT_FIFO_EMPTY);
            }
            out
        } else {
            // return garbage if the fifo is empty
            0
        }
    }

    fn read_status(&mut self) -> u32 {
        log::info!(
            "mdec read status {:?}, remaining_params: {}",
            self.status,
            self.remaining_params
        );
        self.status.bits() | self.remaining_params.wrapping_sub(1) as u32
    }

    fn write_command_params(&mut self, input: u32) {
        // receiveing params
        if let Some(current_cmd) = &mut self.current_cmd {
            match current_cmd {
                MdecCommand::DecodeMacroBlock { data } => {
                    data.push(input as u16);
                    data.push((input >> 16) as u16);
                }
                MdecCommand::SetQuantTable { data, .. } => {
                    let start_i = self.params_ptr * 4;
                    LittleEndian::write_u32(&mut data[start_i..start_i + 4], input);
                }
                MdecCommand::SetScaleTable { data } => {
                    let start_i = self.params_ptr * 2;
                    data[start_i] = input as u16;
                    data[start_i + 1] = (input >> 16) as u16;
                }
            }
            self.remaining_params -= 1;
            self.params_ptr += 1;

            if self.remaining_params == 0 {
                self.status.remove(MdecStatus::COMMAND_BUSY);
                self.exec_current_cmd();
            }
        } else {
            // new command
            let cmd = input >> 29;

            log::info!("mdec command {:?}: {:08X}", cmd, input);
            self.status.insert(MdecStatus::COMMAND_BUSY);
            self.params_ptr = 0;

            // Bit25-28 are copied to STAT.23-26
            self.status.bits |= ((input >> 25) & 0b1111) << 23;

            match cmd {
                // Decode macroblocks
                1 => {
                    self.remaining_params = input as u16;
                    self.current_cmd = Some(MdecCommand::DecodeMacroBlock { data: Vec::new() })
                }
                // Set quant tables
                2 => {
                    let color_and_luminance = input & 1 == 1;
                    log::info!(
                        "mdec set quant tables: color and luminance? {}",
                        color_and_luminance
                    );
                    self.remaining_params = if color_and_luminance {
                        // luminance and color table
                        64 * 2 / 4
                    } else {
                        // luminance table
                        64 / 4
                    };
                    self.current_cmd = Some(MdecCommand::SetQuantTable {
                        color_and_luminance,
                        data: [0; 128],
                    });
                }
                // Set scale tables
                3 => {
                    self.remaining_params = 64 / 2;
                    self.current_cmd = Some(MdecCommand::SetScaleTable { data: [0; 64] });
                }
                // Invalid
                _ => {
                    log::info!("mdec command {} is not valid", cmd);

                    self.remaining_params = input as u16;
                    self.status.remove(MdecStatus::COMMAND_BUSY);
                }
            }
        }
    }

    fn write_control(&mut self, data: u32) {
        log::info!("mdec write control {:032b}", data & 0xF0000000);

        // reset MDEC
        if (data >> 31) & 1 != 0 {
            // clear everything and set `current_block` to 4
            self.status.bits = 0x80040000;
        }

        // enable data in request
        if (data >> 30) & 1 != 0 {
            self.status.insert(MdecStatus::DATA_IN_REQUEST);
        }

        // enable data out request
        if (data >> 29) & 1 != 0 {
            self.status.insert(MdecStatus::DATA_OUT_REQUEST);
        }
    }
}

impl BusLine for Mdec {
    fn read_u32(&mut self, addr: u32) -> u32 {
        match addr & 0xF {
            0 => self.read_response(),
            4 => self.read_status(),
            _ => unreachable!(),
        }
    }

    fn write_u32(&mut self, addr: u32, data: u32) {
        match addr & 0xF {
            0 => self.write_command_params(data),
            4 => self.write_control(data),
            _ => unreachable!(),
        }
    }

    fn read_u16(&mut self, _addr: u32) -> u16 {
        todo!()
    }

    fn write_u16(&mut self, _addr: u32, _data: u16) {
        todo!()
    }

    fn read_u8(&mut self, _addr: u32) -> u8 {
        todo!()
    }

    fn write_u8(&mut self, _addr: u32, _data: u8) {
        todo!()
    }
}
