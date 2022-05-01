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

struct DecodeMacroBlockCommandState {
    // block state
    out: [i16; 64],
    q_scale: u16,
    k: usize,

    // color state
    cr_blk: [i16; 64],
    cb_blk: [i16; 64],
    color_decoding_state: u32,
    color_out: [u32; 256],
}

impl Default for DecodeMacroBlockCommandState {
    fn default() -> Self {
        Self {
            out: [0; 64],
            q_scale: 0,
            k: 0,

            cr_blk: [0; 64],
            cb_blk: [0; 64],
            color_decoding_state: 0,
            color_out: [0; 256],
        }
    }
}

impl DecodeMacroBlockCommandState {
    fn reset_after_block(&mut self) {
        self.out = [0; 64];
        self.k = 0;
        self.q_scale = 0;
    }
}

enum MdecCommand {
    DecodeMacroBlock(DecodeMacroBlockCommandState),
    SetQuantTable { color_and_luminance: bool },
    SetScaleTable,
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
    fn y_to_mono(src: &[i16; 64], signed: bool) -> [u32; 64] {
        let mut out = [0; 64];
        for i in 0..64 {
            let mut y = extend_sign::<10>(src[i] as u16);
            y = y.clamp(-128, 127);
            if !signed {
                y += 128;
            }
            out[i] = (y & 0xFF) as u32;
        }
        out
    }

    fn yuv_to_rgb(
        cr_blk: &[i16; 64],
        cb_blk: &[i16; 64],
        y_blk: &[i16; 64],
        xx: usize,
        yy: usize,
        signed: bool,
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

                if !signed {
                    r += 128;
                    g += 128;
                    b += 128;
                }

                out[(x + xx) + ((y + yy) * 16)] =
                    (r as u32) | ((g as u32) << 8) | ((b as u32) << 16);
            }
        }
    }

    fn real_idct_core(inp_out: &mut [i16; 64], scaletable: &[u16; 64]) {
        let mut tmp = [0; 64];

        // pass 1
        for x in 0..8 {
            for y in 0..8 {
                let mut sum = 0;
                for z in 0..8 {
                    sum += inp_out[x + z * 8] as i64 * (scaletable[y + z * 8] as i16) as i64;
                }
                tmp[x + y * 8] = sum;
            }
        }

        // pass 2
        for x in 0..8 {
            for y in 0..8 {
                let mut sum = 0;
                for z in 0..8 {
                    sum += tmp[y * 8 + z] * (scaletable[x + z * 8] as i16) as i64;
                }
                let t = extend_sign::<9>(((sum >> 32) + ((sum >> 31) & 1)) as u16);
                let t = t.clamp(-128, 127);
                inp_out[x + y * 8] = t as i16;
            }
        }
    }

    // performs `rl_decode_block` but incremently as input come into the MDEC
    // component, returns `true` when its done
    fn rl_decode_block_input(
        new_input: u16,
        qt: &[u8; 64],
        state: &mut DecodeMacroBlockCommandState,
    ) -> bool {
        if state.k == 0 {
            if new_input == 0xFE00 {
                false
            } else {
                let n = new_input;

                state.k = 0;
                state.q_scale = (n >> 10) & 0x3F;

                let mut val = extend_sign::<10>(n & 0x3FF);
                if state.q_scale == 0 {
                    val *= 2;
                } else {
                    val *= qt[0] as i32;
                };

                val = val.clamp(-0x400, 0x3FF);
                let index = if state.q_scale == 0 {
                    state.k
                } else {
                    ZAGZIG[state.k]
                };
                state.out[index] = val as i16;

                state.k += 1;

                false
            }
        } else {
            let n = new_input;

            state.k += ((n >> 10) & 0x3F) as usize;
            if state.k < 64 {
                let mut val =
                    (extend_sign::<10>(n & 0x3FF) * qt[state.k] as i32 * state.q_scale as i32) >> 3;

                if state.q_scale == 0 {
                    val = extend_sign::<10>(n & 0x3FF) * 2;
                }
                val = val.clamp(-0x400, 0x3FF);
                let index = if state.q_scale == 0 {
                    state.k
                } else {
                    ZAGZIG[state.k]
                };
                state.out[index] = val as i16;

                state.k += 1;
            }

            if state.k >= 64 {
                true
            } else {
                false
            }
        }
    }

    fn handle_current_cmd(&mut self, input: u32) {
        if let Some(current_cmd) = &mut self.current_cmd {
            // the purpose of these variables is to own the data,
            // since we need `&mut self` to call `push_to_out_fifo`, and we
            // can't get it while borrowing `current_cmd`
            //
            // set to the idct output for mono decoding
            let mut mono_block_input_done = None;
            // set to the idct output for color decoding
            let mut color_block_input_done = None;
            match current_cmd {
                MdecCommand::DecodeMacroBlock(state) => {
                    match self.status.output_depth() {
                        0 | 1 => {
                            let inp = [input as u16, (input >> 16) as u16];
                            for i in inp {
                                let done = Self::rl_decode_block_input(i, &self.iq_y, state);
                                if done {
                                    Self::real_idct_core(&mut state.out, &self.scaletable);
                                    let out = Self::y_to_mono(
                                        &state.out,
                                        self.status.intersects(MdecStatus::DATA_SIGNED),
                                    );
                                    mono_block_input_done = Some(out);
                                    state.reset_after_block();
                                }
                            }
                        }
                        2 | 3 => {
                            let inp = [input as u16, (input >> 16) as u16];

                            for i in inp {
                                let done = Self::rl_decode_block_input(i, &self.iq_y, state);
                                if done {
                                    Self::real_idct_core(&mut state.out, &self.scaletable);
                                    match state.color_decoding_state {
                                        0 => {
                                            // cr_blk
                                            state.cr_blk = state.out;
                                            state.color_decoding_state = 1;
                                            self.status.set_current_block(5); // Cb
                                        }
                                        1 => {
                                            // cb_blk
                                            state.cb_blk = state.out;
                                            state.color_decoding_state = 2;
                                            self.status.set_current_block(0); // Y1
                                        }
                                        2 => {
                                            // y1
                                            state.color_decoding_state = 3;
                                            Self::yuv_to_rgb(
                                                &state.cr_blk,
                                                &state.cb_blk,
                                                &state.out,
                                                0,
                                                0,
                                                self.status.intersects(MdecStatus::DATA_SIGNED),
                                                &mut state.color_out,
                                            );
                                            self.status.set_current_block(1); // Y2
                                        }
                                        3 => {
                                            // y2
                                            state.color_decoding_state = 4;
                                            Self::yuv_to_rgb(
                                                &state.cr_blk,
                                                &state.cb_blk,
                                                &state.out,
                                                0,
                                                0,
                                                self.status.intersects(MdecStatus::DATA_SIGNED),
                                                &mut state.color_out,
                                            );
                                            self.status.set_current_block(2); // Y3
                                        }
                                        4 => {
                                            // y3
                                            state.color_decoding_state = 5;
                                            Self::yuv_to_rgb(
                                                &state.cr_blk,
                                                &state.cb_blk,
                                                &state.out,
                                                0,
                                                0,
                                                self.status.intersects(MdecStatus::DATA_SIGNED),
                                                &mut state.color_out,
                                            );
                                            self.status.set_current_block(3); // Y4
                                        }
                                        5 => {
                                            // y4
                                            state.color_decoding_state = 0;
                                            Self::yuv_to_rgb(
                                                &state.cr_blk,
                                                &state.cb_blk,
                                                &state.out,
                                                0,
                                                0,
                                                self.status.intersects(MdecStatus::DATA_SIGNED),
                                                &mut state.color_out,
                                            );
                                            color_block_input_done = Some(std::mem::replace(
                                                &mut state.color_out,
                                                [0; 256],
                                            ));
                                        }
                                        _ => unreachable!(),
                                    }
                                    state.reset_after_block();
                                }
                            }
                        }
                        _ => unreachable!(),
                    }
                }
                MdecCommand::SetQuantTable {
                    color_and_luminance,
                } => {
                    if self.params_ptr < 64 / 4 {
                        let start_i = self.params_ptr * 4;
                        LittleEndian::write_u32(&mut self.iq_y[start_i..start_i + 4], input);
                    } else {
                        assert!(*color_and_luminance);
                        let start_i = (self.params_ptr - (64 / 4)) * 4;
                        LittleEndian::write_u32(&mut self.iq_uv[start_i..start_i + 4], input);
                    }
                }
                MdecCommand::SetScaleTable => {
                    let start_i = self.params_ptr * 2;
                    self.scaletable[start_i] = input as u16;
                    self.scaletable[start_i + 1] = (input >> 16) as u16;
                }
            }
            self.remaining_params -= 1;
            self.params_ptr += 1;

            if let Some(out_block) = mono_block_input_done {
                match self.status.output_depth() {
                    0 | 1 => {
                        self.push_to_out_fifo(&out_block);
                    }
                    _ => unreachable!(),
                }
            }
            if let Some(out_block) = color_block_input_done {
                match self.status.output_depth() {
                    2 | 3 => {
                        self.push_to_out_fifo(&out_block);
                    }
                    _ => unreachable!(),
                }
            }
            if self.remaining_params == 0 {
                self.status.remove(MdecStatus::COMMAND_BUSY);
                self.current_cmd = None;
            }
        }
    }
}

impl Mdec {
    fn push_to_out_fifo(&mut self, data: &[u32]) {
        self.status.remove(MdecStatus::DATA_OUT_FIFO_EMPTY);
        match self.status.output_depth() {
            0 => {
                for d in data.chunks(8) {
                    self.data_out_fifo.push_back(u32::from_le_bytes([
                        (d[0] as u8 >> 4) | (d[1] as u8 & 0xF0),
                        (d[2] as u8 >> 4) | (d[3] as u8 & 0xF0),
                        (d[4] as u8 >> 4) | (d[5] as u8 & 0xF0),
                        (d[6] as u8 >> 4) | (d[7] as u8 & 0xF0),
                    ]));
                }
            }
            1 => {
                for d in data.chunks(4) {
                    self.data_out_fifo.push_back(u32::from_le_bytes([
                        d[0] as u8, d[1] as u8, d[2] as u8, d[3] as u8,
                    ]));
                }
            }
            2 => todo!("depth 2"),
            3 => {
                let bit_15 = self.status.intersects(MdecStatus::DATA_OUTPUT_BIT15_SET) as u16;
                for d in data.chunks(2) {
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

    // handles commands params and execution
    fn write_command_params(&mut self, input: u32) {
        // receiveing params
        if self.current_cmd.is_some() {
            self.handle_current_cmd(input);
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
                    self.status.set_current_block(4); // Cr or Y
                    self.current_cmd = Some(MdecCommand::DecodeMacroBlock(Default::default()))
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
                    });
                }
                // Set scale tables
                3 => {
                    self.remaining_params = 64 / 2;
                    self.current_cmd = Some(MdecCommand::SetScaleTable);
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
