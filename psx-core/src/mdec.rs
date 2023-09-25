use std::collections::VecDeque;

use crate::memory::{BusLine, Result};
use bitflags::bitflags;
use byteorder::{ByteOrder, LittleEndian};

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

const DEFAULT_IQ: [u8; 64] = [
    2, 16, 16, 19, 16, 19, 22, 22, 22, 22, 22, 22, 26, 24, 26, 27, 27, 27, 26, 26, 26, 26, 27, 27,
    27, 29, 29, 29, 34, 34, 34, 29, 29, 29, 27, 27, 29, 29, 32, 32, 34, 34, 37, 38, 37, 35, 35, 34,
    35, 38, 38, 40, 40, 40, 48, 48, 46, 46, 56, 56, 58, 69, 69, 83,
];
// note that this is originally i16, but the emulator deals with u16 so its converted
const DEFAULT_SCALETABLE: [u16; 64] = [
    23170, 23170, 23170, 23170, 23170, 23170, 23170, 23170, 32138, 27245, 18204, 6392, 59143,
    47331, 38290, 33397, 30273, 12539, 52996, 35262, 35262, 52996, 12539, 30273, 27245, 59143,
    33397, 47331, 18204, 32138, 6392, 38290, 23170, 42365, 42365, 23170, 23170, 42365, 42365,
    23170, 18204, 33397, 6392, 27245, 38290, 59143, 32138, 47331, 12539, 35262, 30273, 52996,
    52996, 30273, 35262, 12539, 6392, 47331, 27245, 33397, 32138, 38290, 18204, 59143,
];

const fn extend_sign<const N: usize>(x: u16) -> i32 {
    let mask: u32 = (1 << N) - 1;
    let x = x as u32 & mask;
    let sign_extend = (0xFFFFFFFFu32 - mask) * ((x >> (N - 1)) & 1);

    (x | sign_extend) as i32
}

bitflags! {
    #[derive(Default, Debug)]
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
        ((self.bits() & Self::DATA_OUTPUT_DEPTH.bits()) >> 25) as u8
    }

    fn set_current_block(&mut self, block: BlockType) {
        let block = block as u32;
        self.remove(Self::CURRENT_BLOCK);
        *self |= Self::from_bits_retain(block << 16) & Self::CURRENT_BLOCK;
    }
}

struct DecodeMacroBlockCommandState {
    // block state
    rl_out: [i16; 64],
    q_scale: u16,
    k: usize,
    first: bool,

    // color state
    cr_blk: [i16; 64],
    cb_blk: [i16; 64],
    color_decoding_state: u32,
}

impl Default for DecodeMacroBlockCommandState {
    fn default() -> Self {
        Self {
            rl_out: [0; 64],
            q_scale: 0,
            k: 0,
            first: true,

            cr_blk: [0; 64],
            cb_blk: [0; 64],
            color_decoding_state: 0,
        }
    }
}

impl DecodeMacroBlockCommandState {
    fn reset_after_block(&mut self) {
        self.rl_out = [0; 64];
        self.k = 0;
        self.q_scale = 0;
        self.first = true;
    }
}

enum MdecCommand {
    DecodeMacroBlock(Box<DecodeMacroBlockCommandState>),
    SetQuantTable { color_and_luminance: bool },
    SetScaleTable,
}

#[derive(Debug, Clone, Copy)]
pub enum BlockType {
    Y1 = 0,
    Y2,
    Y3,
    Y4,
    YCr, // Y in mono mode, Cr input in color mode
    Cb,
}

#[derive(Clone, Copy)]
pub struct FifoBlockState {
    pub block_type: BlockType,
    pub index: usize,
    pub is_24bit: bool,
}

struct FifoBlock {
    data: [u32; 48],
    size: usize,
    state: FifoBlockState,
}

pub struct Mdec {
    status: MdecStatus,
    remaining_params: u16,
    current_cmd: Option<MdecCommand>,
    params_ptr: usize,

    out_fifo: VecDeque<FifoBlock>,

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

            out_fifo: VecDeque::new(),

            iq_y: DEFAULT_IQ,
            iq_uv: DEFAULT_IQ,
            scaletable: DEFAULT_SCALETABLE,
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
    ) -> [u32; 64] {
        let mut out = [0; 64];
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

                out[x + (y * 8)] = (r as u32) | ((g as u32) << 8) | ((b as u32) << 16);
            }
        }

        out
    }

    fn real_idct_core(inp: &[i16; 64], scaletable: &[u16; 64]) -> [i16; 64] {
        let mut tmp = [0; 64];
        let mut out = [0; 64];

        // pass 1
        for x in 0..8 {
            for y in 0..8 {
                let mut sum = 0;
                for z in 0..8 {
                    sum += inp[x + z * 8] as i64 * (scaletable[y + z * 8] as i16) as i64;
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
                out[x + y * 8] = t as i16;
            }
        }
        out
    }

    // performs `rl_decode_block` but incremently as input come into the MDEC
    // component, returns `true` when its done
    fn rl_decode_block_input(
        new_input: u16,
        qt: &[u8; 64],
        state: &mut DecodeMacroBlockCommandState,
    ) -> bool {
        if new_input == 0xFE00 {
            // if its first, then ignore
            return !state.first;
        }

        let bottom_10 = new_input & 0x3FF;
        let top_6 = (new_input >> 10) & 0x3F;

        if state.first {
            if new_input == 0 {
                return false;
            }
            state.first = false;

            if bottom_10 != 0 {
                let m = if state.q_scale == 0 { 2 } else { qt[0] as i32 };
                let val = extend_sign::<10>(bottom_10) * m;
                state.rl_out[0] = val.clamp(-0x400, 0x3FF) as i16;
            }
            state.q_scale = top_6;
            state.k = 0;
            false
        } else {
            state.k += top_6 as usize + 1;
            if state.k >= 63 {
                return true;
            }
            let i_rev_zig_zag_pos = if state.q_scale == 0 {
                state.k
            } else {
                ZIGZAG[state.k]
            };

            if bottom_10 != 0 {
                let val = if state.q_scale == 0 {
                    extend_sign::<10>(bottom_10) * 2
                } else {
                    (extend_sign::<10>(bottom_10) * qt[state.k] as i32 * state.q_scale as i32 + 4)
                        >> 3
                };
                state.rl_out[i_rev_zig_zag_pos] = val.clamp(-0x400, 0x3FF) as i16;
            }

            false
        }
    }

    fn handle_current_cmd(&mut self, input: u32) {
        if let Some(current_cmd) = &mut self.current_cmd {
            // the purpose of these variables is to own the data,
            // since we need `&mut self` to call `push_to_out_fifo`, and we
            // can't get it while borrowing `current_cmd`
            //
            // set to the idct output for mono decoding
            let mut block_done = None;
            match current_cmd {
                MdecCommand::DecodeMacroBlock(state) => {
                    let inp = [input as u16, (input >> 16) as u16];
                    let mut idct_out = None;
                    for i in inp {
                        let done = Self::rl_decode_block_input(i, &self.iq_y, state);
                        if done {
                            idct_out = Some(Self::real_idct_core(&state.rl_out, &self.scaletable));
                            state.reset_after_block();
                        }
                    }

                    if let Some(idct_out) = idct_out {
                        match self.status.output_depth() {
                            0 | 1 => {
                                let out = Self::y_to_mono(
                                    &idct_out,
                                    self.status.intersects(MdecStatus::DATA_SIGNED),
                                );
                                block_done = Some((out, BlockType::YCr));
                            }
                            2 | 3 => {
                                // This is used to reduce code duplication when
                                // decoding the color blocks.
                                //
                                // (current_block_type, next_block_type, (x,y))
                                let blocks_data = &[
                                    (BlockType::Y1, BlockType::Y2, (0, 0)),
                                    (BlockType::Y2, BlockType::Y3, (8, 0)),
                                    (BlockType::Y3, BlockType::Y4, (0, 8)),
                                    (BlockType::Y4, BlockType::YCr, (8, 8)),
                                ];
                                match state.color_decoding_state {
                                    0 => {
                                        // Cr
                                        state.cr_blk = idct_out;
                                        state.color_decoding_state = 1;
                                        // next block input
                                        self.status.set_current_block(BlockType::Cb);
                                    }
                                    1 => {
                                        // Cb
                                        state.cb_blk = idct_out;
                                        state.color_decoding_state = 2;
                                        // next block input
                                        self.status.set_current_block(BlockType::Y1);
                                    }
                                    _ => {
                                        // Y1..4
                                        let (block_type, next_block_type, (x, y)) =
                                            blocks_data[state.color_decoding_state as usize - 2];
                                        state.color_decoding_state =
                                            (state.color_decoding_state + 1) % 6;
                                        let rgb_block = Self::yuv_to_rgb(
                                            &state.cr_blk,
                                            &state.cb_blk,
                                            &idct_out,
                                            x,
                                            y,
                                            self.status.intersects(MdecStatus::DATA_SIGNED),
                                        );
                                        block_done = Some((rgb_block, block_type));
                                        // next block input
                                        self.status.set_current_block(next_block_type);
                                    }
                                }
                            }
                            _ => unreachable!(),
                        }
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

            if let Some((out_block, block_type)) = block_done.take() {
                self.push_to_out_fifo(out_block, block_type);
            }
            if self.remaining_params == 0 {
                match self.current_cmd {
                    Some(MdecCommand::SetQuantTable {
                        color_and_luminance,
                    }) => {
                        log::info!("color {:?}", self.iq_y);
                        if color_and_luminance {
                            log::info!("lum {:?}", self.iq_uv);
                        }
                    }
                    Some(MdecCommand::SetScaleTable) => {
                        log::info!("scale {:?}", self.scaletable);
                    }
                    _ => {}
                }

                self.status.remove(MdecStatus::COMMAND_BUSY);
                self.current_cmd = None;
            }
        }
    }
}

impl Mdec {
    fn push_to_out_fifo(&mut self, data: [u32; 64], block_type: BlockType) {
        self.status.remove(MdecStatus::DATA_OUT_FIFO_EMPTY);

        let mut out_data = [0u32; 48];
        let size;
        let mut i = 0;
        match self.status.output_depth() {
            0 => {
                // 8 words
                size = 64 / 8;
                for d in data.chunks(8) {
                    out_data[i] = u32::from_le_bytes([
                        (d[0] as u8 >> 4) | (d[1] as u8 & 0xF0),
                        (d[2] as u8 >> 4) | (d[3] as u8 & 0xF0),
                        (d[4] as u8 >> 4) | (d[5] as u8 & 0xF0),
                        (d[6] as u8 >> 4) | (d[7] as u8 & 0xF0),
                    ]);
                    i += 1;
                }
            }
            1 => {
                // 16 words
                size = 64 / 4;
                for d in data.chunks(4) {
                    out_data[i] =
                        u32::from_le_bytes([d[0] as u8, d[1] as u8, d[2] as u8, d[3] as u8]);
                    i += 1;
                }
            }
            2 => {
                // 48 words
                size = 48;
                let mut word_buffer = [0; 4];
                let mut word_buffer_i = 0;
                for color in data {
                    let [r, g, b, _] = color.to_le_bytes();
                    let current_color = [r, g, b];

                    for c in current_color {
                        word_buffer[word_buffer_i] = c;
                        word_buffer_i += 1;
                        if word_buffer_i == 4 {
                            out_data[i] = u32::from_le_bytes(word_buffer);
                            word_buffer_i = 0;
                            i += 1;
                        }
                    }
                }
            }
            3 => {
                // 32 words
                size = 64 / 2;
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

                    out_data[i] = (d1 as u32) | ((d2 as u32) << 16);
                    i += 1;
                }
            }
            _ => unreachable!(),
        }

        self.out_fifo.push_back(FifoBlock {
            data: out_data,
            size,
            state: FifoBlockState {
                block_type,
                index: 0,
                is_24bit: self.status.output_depth() == 2,
            },
        });
    }

    fn read_status(&mut self) -> u32 {
        log::trace!(
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
            self.status |= MdecStatus::from_bits_retain(((input >> 25) & 0b1111) << 23);

            match cmd {
                // Decode macroblocks
                1 => {
                    self.remaining_params = input as u16;
                    self.status.set_current_block(BlockType::YCr); // Cr or Y
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
            self.status = MdecStatus::from_bits_retain(0x80040000);
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

impl Mdec {
    pub fn read_fifo(&mut self) -> u32 {
        if let Some(block) = self.out_fifo.front_mut() {
            let out = block.data[block.state.index];
            block.state.index += 1;
            if block.state.index == block.size {
                self.out_fifo.pop_front();
                if self.out_fifo.is_empty() {
                    self.status.insert(MdecStatus::DATA_OUT_FIFO_EMPTY);
                }
            }
            out
        } else {
            // return garbage if the fifo is empty
            log::warn!("mdec read fifo: fifo is empty");
            0
        }
    }

    pub fn fifo_current_state(&self) -> FifoBlockState {
        if let Some(block) = self.out_fifo.front() {
            block.state
        } else {
            FifoBlockState {
                block_type: BlockType::YCr,
                index: 0,
                is_24bit: false,
            }
        }
    }
}

impl BusLine for Mdec {
    fn read_u32(&mut self, addr: u32) -> Result<u32> {
        let r = match addr & 0xF {
            0 => self.read_fifo(),
            4 => self.read_status(),
            _ => unreachable!(),
        };
        Ok(r)
    }

    fn write_u32(&mut self, addr: u32, data: u32) -> Result<()> {
        match addr & 0xF {
            0 => self.write_command_params(data),
            4 => self.write_control(data),
            _ => unreachable!(),
        }
        Ok(())
    }
}
