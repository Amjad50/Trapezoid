use crate::memory::BusLine;
use bitflags::bitflags;
use byteorder::{ByteOrder, LittleEndian};

bitflags! {
    #[derive(Default)]
    pub struct MdecStatus: u32 {
        const DATA_OUT_FIFO_EMPTY     = 0b10000000_00000000_00000000_00000000;
        const DATA_IN_FIFO_FULL       = 0b01000000_00000000_00000000_00000000;
        const COMMAND_BUSY            = 0b00100000_00000000_00000000_00000000;
        const DATA_IN_REQUEST         = 0b00010000_00000000_00000000_00000000;
        const DATA_OUT_REQUEST        = 0b00001000_00000000_00000000_00000000;
        const DATA_OUTPUT_DEPTH       = 0b00000110_00000000_00000000_00000000;
        const DATA_SIGNED             = 0b00000001_00000000_00000000_00000000;
        const DATA_OUTPUT_BIT15_SET   = 0b00000000_10000000_00000000_00000000;
        const CURRENT_BLOCK           = 0b00000000_00000111_00000000_00000000;
        //const UNUSED_BITS           = 0b00000000_01111000_00000000_00000000;
        //const PARAMS_REMAINING      = 0b00000000_00000000_11111111_11111111;
    }
}

enum MdecCommand {
    DecodeMacroBlock {
        data: Vec<u32>,
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

    iq_y: [u8; 64],
    iq_cb: [u8; 64],
    scaletable: [u16; 64],
}

impl Default for Mdec {
    fn default() -> Self {
        Self {
            status: MdecStatus::default(),
            remaining_params: 0,
            current_cmd: None,
            params_ptr: 0,

            iq_y: [0; 64],
            iq_cb: [0; 64],
            scaletable: [0; 64],
        }
    }
}

impl Mdec {
    fn exec_current_cmd(&mut self) {
        match self.current_cmd.take() {
            Some(MdecCommand::DecodeMacroBlock { data }) => {
                todo!("decode data {:08X?}", data);
            }
            Some(MdecCommand::SetQuantTable {
                color_and_luminance,
                data,
            }) => {
                self.iq_y.copy_from_slice(&data[0..64]);
                if color_and_luminance {
                    self.iq_cb.copy_from_slice(&data[64..128]);
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
    fn read_response(&mut self) -> u32 {
        todo!("read response")
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
                    data.push(input);
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
