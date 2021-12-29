use crate::memory::{interrupts::InterruptRequester, BusLine};
use bitflags::bitflags;

use std::{
    collections::VecDeque,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

bitflags! {
    #[derive(Default)]
    struct FifosStatus: u8 {
        const ADPBUSY                 = 0b00000100;
        /// 1 when empty (triggered before writing 1st byte)
        const PARAMETER_FIFO_EMPTY    = 0b00001000;
        /// 0 when full (triggered after writing 16 bytes)
        const PARAMETER_FIFO_NOT_FULL = 0b00010000;
        /// 0 when empty (triggered after reading LAST byte)
        const RESPONSE_FIFO_NOT_EMPTY = 0b00100000;
        /// 0 when empty (triggered after reading LAST byte)
        const DATA_FIFO_NOT_EMPTY     = 0b01000000;
        /// busy transferring and executing the command
        const BUSY                    = 0b10000000;
    }
}

bitflags! {
    #[derive(Default)]
    struct CdromStatus: u8 {
        const ERROR        = 0b00000001;
        const MOTOR_ON     = 0b00000010;
        const SEEK_ERROR   = 0b00000100;
        const GETID_ERROR  = 0b00001000;
        const SHELL_OPEN   = 0b00010000;
        const READING_DATA = 0b00100000;
        const SEEKING      = 0b01000000;
        const PLAYING      = 0b10000000;
    }
}

pub struct Cdrom {
    index: u8,
    fifo_status: FifosStatus,
    status: CdromStatus,
    interrupt_enable: u8,
    interrupt_flag: u8,
    parameter_fifo: VecDeque<u8>,
    response_fifo: VecDeque<u8>,
    command: Option<u8>,
    /// A way to be able to execute a command through more than one cycle,
    /// The type and design might change later
    command_state: Option<u8>,

    cue_file: Option<PathBuf>,
    cue_file_content: String,
    bin_file_content: Vec<u8>,
}

impl Default for Cdrom {
    fn default() -> Self {
        Self {
            index: 0,
            fifo_status: FifosStatus::PARAMETER_FIFO_EMPTY | FifosStatus::PARAMETER_FIFO_NOT_FULL,
            status: CdromStatus::empty(),
            interrupt_enable: 0,
            interrupt_flag: 0,
            parameter_fifo: VecDeque::new(),
            response_fifo: VecDeque::new(),
            command: None,
            command_state: None,
            cue_file: None,
            // empty vectors are not allocated
            cue_file_content: String::new(),
            bin_file_content: Vec::new(),
        }
    }
}

// file reading and handling
impl Cdrom {
    pub fn set_cue_file<P: AsRef<Path>>(&mut self, cue_file: P) {
        let a = cue_file.as_ref().to_path_buf();
        self.load_cue_file(&a);
        self.cue_file = Some(a);
    }

    fn load_cue_file(&mut self, cue_file: &Path) {
        // start motor
        self.status.insert(CdromStatus::MOTOR_ON);

        // read cue file
        let mut file = fs::File::open(cue_file).unwrap();
        let mut cue_content = String::new();
        file.read_to_string(&mut cue_content).unwrap();
        // parse cue format
        let mut parts = cue_content.split_whitespace();
        assert!(parts.next().unwrap() == "FILE");
        // remove quotes
        let bin_file_name = parts.next().unwrap().replace("\"", "");
        assert!(parts.next().unwrap() == "BINARY");
        assert!(parts.next().unwrap() == "TRACK");
        assert!(parts.next().unwrap() == "01");
        assert!(parts.next().unwrap() == "MODE2/2352");
        assert!(parts.next().unwrap() == "INDEX");
        assert!(parts.next().unwrap() == "01");
        assert!(parts.next().unwrap() == "00:00:00");

        // load bin file
        let bin_file_path = cue_file.parent().unwrap().join(bin_file_name);
        log::info!("Loading bin file: {:?}", bin_file_path);
        let mut file = fs::File::open(bin_file_path).unwrap();
        // read to new vector
        let mut bin_file_content = Vec::new();
        file.read_to_end(&mut bin_file_content).unwrap();
        self.cue_file_content = cue_content;
        self.bin_file_content = bin_file_content;
    }
}

// clocking and commands
impl Cdrom {
    pub fn clock(&mut self, interrupt_requester: &mut impl InterruptRequester) {
        self.execute_next_command(interrupt_requester);
    }

    fn execute_next_command(&mut self, interrupt_requester: &mut impl InterruptRequester) {
        if let Some(cmd) = self.command {
            match (cmd, self.command_state) {
                (0x01, None) => {
                    // GetStat
                    // TODO: handle errors
                    assert!(self.status.bits & 0b101 == 0);

                    self.write_to_response_fifo(self.status.bits);
                    self.request_interrupt_0_7(3);

                    self.reset_command();
                }
                (0x19, None) => {
                    // Test
                    let test_code = self.read_next_parameter().unwrap();
                    self.execute_test(test_code);

                    self.reset_command();
                }
                (0x1A, None) => {
                    // GetID FIRST

                    self.write_to_response_fifo(self.status.bits);
                    self.request_interrupt_0_7(3);
                    // any data for now, just to proceed to SECOND
                    self.command_state = Some(0);
                }
                (0x1A, Some(_)) => {
                    // GetID SECOND

                    // wait for acknowledge
                    if self.interrupt_flag & 7 == 0 {
                        // TODO: rewrite GetID implementation to fill
                        //       all the details correctly from the state of the cdrom
                        let (response, interrupt) = if self.cue_file.is_some() {
                            // last byte is the region code identifier
                            // A(0x41): NTSC
                            // E(0x45): PAL
                            // I(0x49): JP
                            (&[0x02, 0x00, 0x20, 0x00, 0x53, 0x43, 0x45, 0x41], 2)
                        } else {
                            //  5 interrupt means error
                            (&[0x08, 0x40, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00], 5)
                        };

                        self.write_slice_to_response_fifo(response);

                        self.request_interrupt_0_7(interrupt);

                        self.reset_command();
                    }
                }
                (0x1E, None) => {
                    // GetToc FIRST

                    self.write_to_response_fifo(self.status.bits);
                    self.request_interrupt_0_7(3);
                    // any data for now, just to proceed to SECOND
                    self.command_state = Some(0);
                }
                (0x1E, Some(_)) => {
                    // GetToc SECOND

                    // wait for acknowledge
                    if self.interrupt_flag & 7 == 0 {
                        self.write_to_response_fifo(self.status.bits);
                        self.request_interrupt_0_7(2);
                        self.reset_command();
                    }
                }
                _ => todo!("cmd={:02X},state={:?}", cmd, self.command_state),
            }

            // fire irq only if the interrupt is enabled
            if self.interrupt_flag & self.interrupt_enable != 0 {
                interrupt_requester.request_cdrom();
            }
        }
    }

    fn execute_test(&mut self, test_code: u8) {
        match test_code {
            0x20 => {
                self.write_slice_to_response_fifo(&[0x99u8, 0x02, 0x01, 0xC3]);
                self.request_interrupt_0_7(3);
            }
            _ => todo!(),
        }
    }

    fn put_command(&mut self, cmd: u8) {
        self.command = Some(cmd);
        self.command_state = None;
        self.fifo_status.insert(FifosStatus::BUSY);
    }

    fn reset_command(&mut self) {
        self.command = None;
        self.command_state = None;
        self.fifo_status.remove(FifosStatus::BUSY);
    }
}

impl Cdrom {
    fn read_index_status(&self) -> u8 {
        self.index | self.fifo_status.bits
    }

    fn write_interrupt_enable_register(&mut self, data: u8) {
        self.interrupt_enable = data & 0x1F;
        log::info!(
            "2.1 write interrupt enable register value={:02X}",
            self.interrupt_enable
        );
    }

    fn read_interrupt_enable_register(&self) -> u8 {
        (self.interrupt_enable & 0x1F) | 0xE0
    }

    fn write_interrupt_flag_register(&mut self, data: u8) {
        log::info!("3.1 write interrupt flag register value={:02X}", data);
        let interrupts_flag_to_ack = data & 0x1F;
        self.interrupt_flag &= !interrupts_flag_to_ack;

        if data & 0x40 != 0 {
            self.reset_parameter_fifo();
        }
    }

    fn read_interrupt_flag_register(&self) -> u8 {
        (self.interrupt_flag & 0x1F) | 0xE0
    }

    fn write_command_register(&mut self, data: u8) {
        log::info!("1.0 writing to command register cmd={:02X}", data);
        self.put_command(data)
    }

    fn reset_parameter_fifo(&mut self) {
        self.fifo_status.insert(FifosStatus::PARAMETER_FIFO_EMPTY);
        self.fifo_status
            .insert(FifosStatus::PARAMETER_FIFO_NOT_FULL);
        self.parameter_fifo.clear();
    }

    fn write_to_parameter_fifo(&mut self, data: u8) {
        if self.parameter_fifo.is_empty() {
            self.fifo_status.remove(FifosStatus::PARAMETER_FIFO_EMPTY);
        } else if self.parameter_fifo.len() == 15 {
            self.fifo_status
                .remove(FifosStatus::PARAMETER_FIFO_NOT_FULL);
        }
        log::info!("2.0 writing to parameter fifo={:02X}", data);

        self.parameter_fifo.push_back(data);
    }

    fn read_next_parameter(&mut self) -> Option<u8> {
        let out = self.parameter_fifo.pop_front();
        if self.parameter_fifo.is_empty() {
            self.fifo_status.insert(FifosStatus::PARAMETER_FIFO_EMPTY);
        } else if self.parameter_fifo.len() == 15 {
            self.fifo_status
                .insert(FifosStatus::PARAMETER_FIFO_NOT_FULL);
        }

        out
    }

    fn write_to_response_fifo(&mut self, data: u8) {
        if self.response_fifo.is_empty() {
            self.fifo_status
                .insert(FifosStatus::RESPONSE_FIFO_NOT_EMPTY);
        }
        log::info!("writing to response fifo={:02X}", data);

        self.response_fifo.push_back(data);
    }

    fn write_slice_to_response_fifo(&mut self, data: &[u8]) {
        if self.response_fifo.is_empty() {
            self.fifo_status
                .insert(FifosStatus::RESPONSE_FIFO_NOT_EMPTY);
        }
        log::info!("writing to response fifo={:02X?}", data);

        self.response_fifo.extend(data);
    }

    fn read_next_response(&mut self) -> u8 {
        let out = self.response_fifo.pop_front();

        if self.response_fifo.is_empty() {
            self.fifo_status
                .remove(FifosStatus::RESPONSE_FIFO_NOT_EMPTY);
        }

        // TODO: handle reading while being empty
        out.unwrap()
    }

    fn request_interrupt_0_7(&mut self, int_value: u8) {
        self.interrupt_flag &= !0x7;
        self.interrupt_flag |= int_value & 0x7;
    }
}

impl BusLine for Cdrom {
    fn read_u32(&mut self, _addr: u32) -> u32 {
        todo!()
    }

    fn write_u32(&mut self, _addr: u32, _data: u32) {
        todo!()
    }

    fn read_u16(&mut self, addr: u32) -> u16 {
        assert!(addr == 2);

        todo!()
    }

    fn write_u16(&mut self, _addr: u32, _data: u16) {
        todo!()
    }

    fn read_u8(&mut self, addr: u32) -> u8 {
        match addr {
            0 => self.read_index_status(),
            1 => self.read_next_response(),
            2 => todo!("read 2 data fifo"),
            3 => match self.index & 1 {
                0 => self.read_interrupt_enable_register(),
                1 => self.read_interrupt_flag_register(),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    }

    fn write_u8(&mut self, addr: u32, data: u8) {
        match addr {
            0 => {
                self.index = data & 3;
            }
            1 => match self.index {
                0 => self.write_command_register(data),
                1 => todo!("write 1.1 Sound Map Data Out"),
                2 => todo!("write 1.2 Sound Map Coding Info"),
                3 => todo!("write 1.3 vol stuff"),
                _ => unreachable!(),
            },
            2 => match self.index {
                0 => self.write_to_parameter_fifo(data),
                1 => self.write_interrupt_enable_register(data),
                2 => todo!("write 2.2 vol stuff"),
                3 => todo!("write 2.3 vol stuff"),
                _ => unreachable!(),
            },
            3 => match self.index {
                0 => todo!("write 3.0 request register"),
                1 => self.write_interrupt_flag_register(data),
                2 => todo!("write 3.2 vol stuff"),
                3 => todo!("write 3.3 vol stuff"),
                _ => unreachable!(),
            },
            _ => unreachable!(),
        }
    }
}
